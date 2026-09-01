//! A Rust client for the MLS consensus program.
//!
//! Reading and writing are separate objects: a [`ConvoStateReader`] is just an RPC connection,
//! while a [`ConvoStateWriter`] holds a wallet, and therefore signing keys. Keeping them
//! apart means a component that only follows a group cannot be handed the ability to commit to
//! it.
//!
//! ```no_run
//! use mls_consensus_client::{ConvoStateReader, ConvoStateWriter};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! const SEQUENCER: &str = "http://localhost:3040";
//!
//! let writer = ConvoStateWriter::new(SEQUENCER).await?;
//! let group = writer.account();
//! writer.deploy().await?;
//! writer.initialize().await?;
//! writer.commit(0, [7; 32].into()).await?;
//!
//! let reader = ConvoStateReader::connect(SEQUENCER, group)?;
//! let state = reader.get().await?.expect("just initialized");
//! assert_eq!(state.epoch_history, vec![[7; 32].into()]);
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "blocking")]
pub mod blocking;
mod error;
mod reader;
mod writer;

#[cfg(feature = "blocking")]
pub use blocking::{BlockingReader, BlockingWriter};
pub use error::{ConvoStateReadError, ConvoStateWriteError};
pub use mls_consensus_core::{ConvoState, EpochFingerprint, Instruction};
pub use reader::ConvoStateReader;
pub use writer::{ConvoStateWriter, TxHash};

/// Re-exported so callers can name accounts without depending on `lee` directly.
pub use lee::AccountId;

/// The program's on-chain identity.
pub use mls_consensus_methods::CONVO_STATE_ID as PROGRAM_ID;
