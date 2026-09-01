use std::time::Duration;

use common::HashType;
use lee::AccountId;
use mls_consensus_core::ConvoStateError;

#[derive(Debug, thiserror::Error)]
pub enum ConvoStateReadError {
    #[error("sequencer request failed")]
    Rpc(#[from] sequencer_service_rpc::ClientError),

    #[error("invalid sequencer URL")]
    Url(#[from] url::ParseError),

    /// Bytes are present but are not a `ConvoState` — the account belongs to something else.
    #[error("account {account} does not hold a valid ConvoState")]
    Invalid {
        account: AccountId,
        #[source]
        source: ConvoStateError,
    },

    #[error("{account} holds no group state")]
    NoState { account: AccountId },

    /// [`crate::ConvoStateReader::await_version`] gave up. The group may still get there later:
    /// a transaction can be accepted long before its block is produced.
    #[error("{account} did not reach version {version} within {waited:?}")]
    NotReached {
        account: AccountId,
        version: u64,
        waited: std::time::Duration,
    },

    #[cfg(feature = "blocking")]
    #[error("could not start a runtime for the blocking client")]
    Runtime(#[source] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ConvoStateWriteError {
    #[error(transparent)]
    Read(#[from] ConvoStateReadError),

    #[error("{account} is already initialized, at version {version}")]
    AlreadyInitialized { account: AccountId, version: u64 },

    /// Another member committed first. Re-read and rebuild against `actual`.
    #[error("version conflict on {account}: expected {expected}, the group is at {actual}")]
    VersionConflict {
        account: AccountId,
        expected: u64,
        actual: u64,
    },

    /// Submitted, but not applied by the time we stopped waiting — and it may still land.
    /// Only a transaction from this account can consume its nonce, so until one does, this one
    /// stays valid. Re-read before deciding anything; do not assume it failed.
    #[error("transaction {tx} for {account} is still pending after {waited:?}")]
    StillPending {
        account: AccountId,
        tx: HashType,
        expected_version: u64,
        waited: Duration,
    },

    /// Boxed: 112 bytes inline, which on its own put this enum on `clippy::result_large_err`'s
    /// threshold and made every `Result` in the crate pay for it.
    #[error("transaction failed")]
    Transaction(#[source] Box<wallet::ExecutionFailureKind>),

    /// Boxed for the same reason.
    #[error("could not encode the instruction")]
    Encoding(#[source] Box<lee::error::LeeError>),

    #[error("program error: {0}")]
    Program(String),
}

// Hand-written rather than `#[from]`, which would generate `From<Box<..>>` and break `?` at
// every call site that has an unboxed error in hand.
impl From<wallet::ExecutionFailureKind> for ConvoStateWriteError {
    fn from(error: wallet::ExecutionFailureKind) -> Self {
        Self::Transaction(Box::new(error))
    }
}

impl From<lee::error::LeeError> for ConvoStateWriteError {
    fn from(error: lee::error::LeeError) -> Self {
        Self::Encoding(Box::new(error))
    }
}
