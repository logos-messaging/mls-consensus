use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::{Data, data::DataTooBigError};
use serde::{Deserialize, Serialize};

/// A digest of a member's MLS `GroupContext` once it has applied a commit.
///
/// Every member is supposed to hold an identical `GroupContext`, so hashing it turns "are we all
/// in the same epoch?" into a 32-byte comparison. A member whose own digest differs from the one
/// the group recorded has diverged and knows to stop; detecting that is the only reason this
/// value is on chain.
///
/// The program neither computes nor checks one. Which hash the members use is theirs to choose,
/// so long as it is truncated or re-hashed to fit the 32 bytes here.

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EpochFingerprint([u8; 32]);

impl EpochFingerprint {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl std::fmt::Debug for EpochFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpochFingerprint")
            .field("bytes", &hex(self.as_bytes()))
            .finish()
    }
}

impl From<[u8; 32]> for EpochFingerprint {
    fn from(value: [u8; 32]) -> Self {
        EpochFingerprint(value)
    }
}

/// The consensus view of one MLS group: the epochs it has passed through, and where that leaves
/// it.
///
/// Borsh-encoded into the account's `data` field, so a reader needs nothing but the account —
/// no index to rebuild, no history to replay.
///
/// Deliberately *not* the MLS group state: no key material, no ratchet tree, no plaintext.
/// Members derive all of that themselves from commits they exchange off chain.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ConvoState {
    /// How many epochs the group has recorded, and the compare-and-swap token guarding the next.
    ///
    /// Starts at 0 and rises by one per accepted [`Instruction::Commit`], so it always equals
    /// `epoch_history.len()`.
    pub version: u64,

    /// Every epoch the group has passed through, oldest first; the last is the current one.
    ///
    /// The chain rather than just its head, so a member returning after a gap can find the last
    /// epoch it recognizes and confirm the current one descends from it.
    ///
    /// TODO: Truncate history; epoch_history grows infinitely for now.
    pub epoch_history: Vec<EpochFingerprint>,
}

impl std::fmt::Debug for ConvoState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConvoState")
            .field("version", &self.version)
            .field(
                "epoch_history",
                &self
                    .epoch_history
                    .iter()
                    .map(|fingerprint| hex(fingerprint.as_bytes()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for ConvoState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConvoState {
    /// The state a group starts in: no epochs, and version 0 — the version its first
    /// [`Instruction::Commit`] must name.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: 0,
            epoch_history: vec![],
        }
    }

    /// Ensures a ConvoState is valid and meets all invariants
    pub fn validate(&self) -> Result<(), ConvoStateError> {
        if self.epoch_history.len() != self.version as usize {
            return Err(ConvoStateError::Malformed);
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The complete set of transitions the program will perform.
///
/// Borsh-encoded by the client and decoded by the guest, so both must be built against the same
/// revision of this crate. Reading needs no instruction: the state *is* the account's data.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Claim an uninitialized account for this program and write the empty state.
    ///
    /// Takes no arguments because a group starts at version 0 and every parameter it has is
    /// established by its first commit. Refusing an account that already holds something is what
    /// keeps this from being a second way to spell `Commit`.
    Initialize {},

    /// Advance the group by one MLS commit, if it is still in the epoch the caller saw.
    Commit {
        /// The version this commit expects to replace. A mismatch means another member
        /// committed first, and the transaction is rejected rather than clobbering the new
        /// head.
        expected_version: u64,

        /// The epoch this commit lands the group in, appended to [`ConvoState::epoch_history`].
        epoch_fingerprint: EpochFingerprint,
    },
}

/// Errors produced while moving a [`ConvoState`] in or out of an account's `data` field.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConvoStateError {
    /// The account's `data` did not decode as a [`ConvoState`]: another program's account, or
    /// an incompatible revision of this crate. The (unreachable) encode failure maps here too.
    #[error("account data is not a valid ConvoState")]
    Malformed,

    /// The encoded state is larger than a LEE account's `data` field can hold.
    #[error("ConvoState is too big to encode into an account")]
    TooBigForAccount,
}

impl TryFrom<&Data> for ConvoState {
    type Error = ConvoStateError;

    fn try_from(data: &Data) -> Result<Self, Self::Error> {
        Self::try_from_slice(data.as_ref()).map_err(|_| ConvoStateError::Malformed)
    }
}

impl TryFrom<&ConvoState> for Data {
    type Error = ConvoStateError;

    fn try_from(convo_state: &ConvoState) -> Result<Self, Self::Error> {
        let bytes = borsh::to_vec(convo_state).map_err(|_| ConvoStateError::Malformed)?;
        Self::try_from(bytes).map_err(|DataTooBigError| ConvoStateError::TooBigForAccount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConvoState {
        ConvoState {
            version: 2,
            epoch_history: vec![EpochFingerprint([1; 32])],
        }
    }

    #[test]
    fn a_new_group_is_empty_at_version_zero() {
        let state = ConvoState::new();

        assert_eq!(state.version, 0);
        assert!(state.epoch_history.is_empty());
    }

    /// Client and program both encode `data` with this crate, so a round trip is their contract.
    #[test]
    fn a_state_round_trips_through_account_data() {
        let state = sample();
        let data = Data::try_from(&state).expect("state should encode");

        assert_eq!(
            ConvoState::try_from(&data).expect("data should decode"),
            state
        );
    }

    #[test]
    fn data_that_is_not_a_state_is_malformed() {
        let data = Data::try_from(b"not a ConvoState".to_vec()).expect("small enough");

        assert_eq!(
            ConvoState::try_from(&data).unwrap_err(),
            ConvoStateError::Malformed
        );
    }

    /// The client encodes instructions and the guest decodes them; the shapes must agree.
    #[test]
    fn an_instruction_round_trips_through_borsh() {
        let instruction = Instruction::Commit {
            expected_version: 7,
            epoch_fingerprint: EpochFingerprint([3; 32]),
        };

        let bytes = borsh::to_vec(&instruction).expect("instruction should encode");

        assert_eq!(
            Instruction::try_from_slice(&bytes).expect("instruction should decode"),
            instruction
        );
    }
}
