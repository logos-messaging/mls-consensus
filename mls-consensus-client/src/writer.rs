use std::time::Duration;

pub use common::HashType as TxHash;
use lee::{AccountId, program::Program};
use mls_consensus_core::{ConvoState, EpochFingerprint, Instruction};
use mls_consensus_methods::{CONVO_STATE_ELF, CONVO_STATE_ID};
use url::Url;
use wallet::{
    AccountIdentity, WalletCore,
    config::{SequencerConnectionData, WalletConfigOverrides},
};

use crate::{ConvoStateReadError, ConvoStateReader, ConvoStateWriteError};

struct GroupManagementKey {
    wallet: WalletCore,
    sequencer: String,
    account: AccountId,
}

impl GroupManagementKey {
    pub async fn new(sequencer: &str) -> Result<Self, ConvoStateWriteError> {
        let home = tempfile::tempdir().map_err(|e| ConvoStateWriteError::Program(e.to_string()))?;

        // This Wallets persisted data is dropped once this function goes out of scope (TempDir).
        // WalletCore does not support ephemeral keys, and this is a work around for POC.
        let (mut wallet, _mnemonic) = WalletCore::new_init_storage(
            home.path().join("wallet_config.json"),
            home.path().join("storage.json"),    // never written
            home.path().join("statistics.json"), // never written
            Some(overrides(sequencer)?),
            "",
        )
        .await
        .map_err(|e| ConvoStateWriteError::Program(e.to_string()))?;

        let (account, _chain_index) = wallet.create_new_account_public(None);

        Ok(Self {
            wallet,
            sequencer: sequencer.to_owned(),
            account,
        })
    }

    pub fn sequencer(&self) -> String {
        self.sequencer.clone()
    }

    pub const fn wallet(&self) -> &WalletCore {
        &self.wallet
    }
}

/// Points a wallet at `sequencer`, whatever its config file says.
fn overrides(sequencer: &str) -> Result<WalletConfigOverrides, ConvoStateWriteError> {
    Ok(WalletConfigOverrides {
        sequencers: Some(vec![SequencerConnectionData {
            sequencer_addr: sequencer
                .parse::<Url>()
                .map_err(|e| ConvoStateWriteError::Program(e.to_string()))?,
            basic_auth: None,
        }]),
        ..WalletConfigOverrides::default()
    })
}

/// What happened to a commit submitted for version `expected_version`.
enum CommitVerdict {
    /// The slot holds our fingerprint.
    Landed,
    /// Someone else's fingerprint is in the slot; ours can never apply now.
    Superseded { actual_version: u64 },
    /// Nobody has taken the slot yet.
    Undecided,
}

/// Writes group state. Holds the group's management key, and therefore signing keys.
pub struct ConvoStateWriter {
    key: GroupManagementKey,
}

impl ConvoStateWriter {
    pub const VERDICT_INTERVAL: Duration = Duration::from_secs(2);

    /// How many times it looks before giving up.
    pub const VERDICT_ATTEMPTS: u32 = 30;

    /// Connects to `sequencer` with a group-management wallet of its own.
    ///
    /// The wallet is ephemeral: its keys live in memory for the life of this writer and are
    /// never written to disk, so a group created here cannot be committed to by a later
    /// process. That is the whole account model for now.
    pub async fn new(sequencer: &str) -> Result<Self, ConvoStateWriteError> {
        Ok(Self::new_with_group(
            GroupManagementKey::new(sequencer).await?,
        ))
    }

    #[must_use]
    const fn new_with_group(group_mgmt_key: GroupManagementKey) -> Self {
        Self {
            key: group_mgmt_key,
        }
    }

    /// The account this writer's group lives in.
    ///
    /// Derived when the writer is built, and the only handle a reader needs — hand it to
    /// [`ConvoStateReader::connect`] to follow the group from code that holds no keys.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.key.account
    }

    /// A keyless reader on the same sequencer, following this writer's account.
    pub fn reader(&self) -> Result<ConvoStateReader, ConvoStateReadError> {
        ConvoStateReader::connect(&self.key.sequencer(), self.key.account)
    }

    /// Deploys the program. Needed once per zone per version of the program.
    pub async fn deploy(&self) -> Result<TxHash, ConvoStateWriteError> {
        self.key
            .wallet()
            .send_program_deployment_transaction(CONVO_STATE_ELF.to_vec())
            .await
            .map_err(|e| ConvoStateWriteError::Program(e.to_string()))
    }

    /// Creates the group, claiming an uninitialized account for the program.
    pub async fn initialize(&self) -> Result<TxHash, ConvoStateWriteError> {
        if let Some(existing) = self.reader()?.get().await? {
            return Err(ConvoStateWriteError::AlreadyInitialized {
                account: self.key.account,
                version: existing.version,
            });
        }

        let tx = self
            .send(self.key.account, Instruction::Initialize {})
            .await?;

        Ok(tx)
    }

    pub async fn commit(
        &self,
        expected_version: u64,
        epoch_fingerprint: EpochFingerprint,
    ) -> Result<(), ConvoStateWriteError> {
        self.check_version(expected_version).await?;

        let tx = self
            .send(
                self.key.account,
                Instruction::Commit {
                    expected_version,
                    epoch_fingerprint,
                },
            )
            .await?;

        let verdict = self
            .await_verdict(expected_version, epoch_fingerprint)
            .await?;

        match verdict {
            CommitVerdict::Landed => Ok(()),
            CommitVerdict::Superseded { actual_version } => {
                Err(ConvoStateWriteError::VersionConflict {
                    account: self.key.account,
                    expected: expected_version,
                    actual: actual_version,
                })
            }
            CommitVerdict::Undecided => Err(ConvoStateWriteError::StillPending {
                account: self.key.account,
                tx,
                expected_version,
                waited: Self::VERDICT_INTERVAL * Self::VERDICT_ATTEMPTS,
            }),
        }
    }

    /// The program enforces this too — checking here turns an on-chain rejection into a typed
    /// error, before a transaction is spent.
    async fn check_version(&self, expected: u64) -> Result<(), ConvoStateWriteError> {
        let state = self
            .reader()?
            .get()
            .await?
            .ok_or(ConvoStateReadError::NoState {
                account: self.key.account,
            })?;

        if state.version != expected {
            return Err(ConvoStateWriteError::VersionConflict {
                account: self.key.account,
                expected,
                actual: state.version,
            });
        }

        Ok(())
    }

    /// Determine the outcome of submitting the transaction.
    async fn await_verdict(
        &self,
        expected_version: u64,
        mine: EpochFingerprint,
    ) -> Result<CommitVerdict, ConvoStateWriteError> {
        let reader = self.reader()?;

        for _ in 0..Self::VERDICT_ATTEMPTS {
            tokio::time::sleep(Self::VERDICT_INTERVAL).await;

            let Some(state) = reader.get().await? else {
                continue; // not initialized yet as far as this read can see
            };

            match classify(&state, expected_version, mine) {
                // Nobody has taken the slot yet — keep waiting.
                CommitVerdict::Undecided => continue,
                decided => return Ok(decided),
            }
        }

        // Timeout reached and still no verdict
        Ok(CommitVerdict::Undecided)
    }

    /// The wallet supplies the nonce and signature, which is what makes the program see
    /// `is_authorized == true`.
    async fn send(
        &self,
        account: AccountId,
        instruction: Instruction,
    ) -> Result<TxHash, ConvoStateWriteError> {
        let instruction_data = Program::serialize_instruction(instruction)?;

        self.key
            .wallet()
            .send_pub_tx(
                vec![AccountIdentity::Public(account)],
                instruction_data,
                CONVO_STATE_ID,
            )
            .await
            .map_err(ConvoStateWriteError::from)
    }
}

/// A commit built for version `V` lands at `epoch_history[V]`, and that slot is filled
/// exactly once — so whoever is in it is the answer.
fn classify(state: &ConvoState, expected_version: u64, mine: EpochFingerprint) -> CommitVerdict {
    let slot = usize::try_from(expected_version)
        .ok()
        .and_then(|index| state.epoch_history.get(index));

    match slot {
        None => CommitVerdict::Undecided,
        Some(&landed) if landed == mine => CommitVerdict::Landed,
        Some(_) => CommitVerdict::Superseded {
            actual_version: state.version,
        },
    }
}

impl std::fmt::Debug for ConvoStateWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConvoStateWriter")
            .field("sequencer", &self.key.sequencer())
            .finish_non_exhaustive()
    }
}
