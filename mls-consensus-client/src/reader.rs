use std::time::Duration;

use lee::AccountId;
use mls_consensus_core::{ConvoState, EpochFingerprint};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};

use crate::ConvoStateReadError;

/// Reads one group's state. Needs no keys and no wallet.
///
/// The account is fixed when the reader is built, so a reader follows exactly one group.
#[derive(Clone)]
pub struct ConvoStateReader {
    pub client: SequencerClient,
    pub account: AccountId,
}

impl ConvoStateReader {
    /// How often [`Self::await_version`] looks for a new version.
    pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

    /// How many times it looks before giving up.
    pub const POLL_ATTEMPTS: u32 = 30;

    /// Connects to a sequencer, e.g. `"http://localhost:3040"`. No I/O until the first request.
    pub fn connect(sequencer: &str, account: AccountId) -> Result<Self, ConvoStateReadError> {
        let url: url::Url = sequencer.parse()?;
        let client = SequencerClientBuilder::default()
            .build(url)
            .map_err(ConvoStateReadError::Rpc)?;

        Ok(Self { client, account })
    }

    #[must_use]
    pub const fn from_client(client: SequencerClient, account: AccountId) -> Self {
        Self { client, account }
    }

    /// The account this reader follows.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.account
    }

    /// `Ok(None)` means the account holds nothing yet; bytes that are present but undecodable
    /// are an error, since reporting them as empty would invite overwriting another program.
    pub async fn get(&self) -> Result<Option<ConvoState>, ConvoStateReadError> {
        let state = self.client.get_account(self.account).await?;

        if state.data.as_ref().is_empty() {
            return Ok(None);
        }

        ConvoState::try_from(&state.data)
            .map(Some)
            .map_err(|source| ConvoStateReadError::Invalid {
                account: self.account,
                source,
            })
    }

    /// The number to pass as a commit's `expected_version`.
    pub async fn version(&self) -> Result<Option<u64>, ConvoStateReadError> {
        Ok(self.get().await?.map(|state| state.version))
    }

    /// The fingerprint of the epoch the group is in now — the head of its history.
    pub async fn head(&self) -> Result<Option<EpochFingerprint>, ConvoStateReadError> {
        Ok(self
            .get()
            .await?
            .and_then(|state| state.epoch_history.last().copied()))
    }

    /// Polls until the group reaches `version`.
    ///
    /// A submitted transaction is not an applied one: the sequencer accepts it, then the program
    /// runs and the block is produced. A read straight after a write can still see the old
    /// state, which is what this exists to wait out — so it looks after the first interval, not
    /// before it.
    ///
    /// Gives up after [`Self::POLL_ATTEMPTS`] × [`Self::POLL_INTERVAL`] with
    /// [`ConvoStateReadError::NotReached`], which does not mean the group never gets there.
    pub async fn await_version(&self, version: u64) -> Result<ConvoState, ConvoStateReadError> {
        for _ in 0..Self::POLL_ATTEMPTS {
            tokio::time::sleep(Self::POLL_INTERVAL).await;

            if let Some(state) = self.get().await?
                && state.version == version
            {
                return Ok(state);
            }
        }

        Err(ConvoStateReadError::NotReached {
            account: self.account,
            version,
            waited: Self::POLL_INTERVAL * Self::POLL_ATTEMPTS,
        })
    }
}

impl std::fmt::Debug for ConvoStateReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConvoStateReader")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}
