//! Synchronous views, for callers with no runtime of their own.
//!
//! Work is handed to the owned runtime over a channel rather than run with `Runtime::block_on`,
//! which panics when called from inside another runtime. So these methods are safe to call from
//! anywhere — but calling one from an async task still blocks that task's thread. Async callers
//! want [`ConvoStateReader`] and [`ConvoStateWriter`].

use std::{
    future::Future,
    sync::{Arc, mpsc},
};

use lee::AccountId;
use mls_consensus_core::{ConvoState, EpochFingerprint};
use tokio::runtime::{Builder, Runtime};

use crate::{
    ConvoStateReadError, ConvoStateReader, ConvoStateWriteError, ConvoStateWriter, TxHash,
};

/// One worker, because a blocking caller can only have one call in flight. The multi-threaded
/// flavour rather than current-thread so anything the client spawns keeps running between calls.
fn new_runtime(name: &str) -> Result<Runtime, std::io::Error> {
    Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name(name)
        .build()
}

/// Runs `future` on `runtime` and blocks the calling thread until it resolves.
///
/// The hand-off is what makes this safe from inside another runtime: `Runtime::block_on` would
/// panic there, while spawning and parking on a channel only blocks the calling thread.
fn block_on<T: Send + 'static>(
    runtime: &Runtime,
    future: impl Future<Output = T> + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();

    runtime.spawn(async move {
        // Send can only fail if the receiver is gone, and it cannot be: the thread that owns
        // it is parked on `recv` below until this value arrives.
        let _ = tx.send(future.await);
    });

    rx.recv().expect("the task panicked without a value")
}

/// Same, for a future that cannot be proven `Send`.
///
/// `make` runs on a blocking-pool thread and its future is driven there, so the future never
/// crosses a thread boundary and needs no `Send` bound. The writer's futures need this: the
/// sequencer's RPC client leaks a higher-ranked lifetime through them, which leaves
/// `&ConvoStateWriter: Send` unprovable even though `ConvoStateWriter` is `Sync`.
///
/// `Handle::block_on` is legal on a blocking-pool thread — it is not an async context — and the
/// runtime's worker keeps driving I/O and timers while this thread parks.
fn block_on_local<F, Fut, T>(runtime: &Runtime, make: F) -> T
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T>,
    T: Send + 'static,
{
    let handle = runtime.handle().clone();
    let (tx, rx) = mpsc::channel();

    runtime.spawn_blocking(move || {
        let _ = tx.send(handle.block_on(make()));
    });

    rx.recv().expect("the task panicked without a value")
}

/// Cheap to clone: clones share one runtime and one connection.
#[derive(Clone)]
pub struct BlockingReader {
    reader: ConvoStateReader,
    runtime: Arc<Runtime>,
}

impl BlockingReader {
    pub fn connect(sequencer: &str, account: AccountId) -> Result<Self, ConvoStateReadError> {
        Self::with_reader(ConvoStateReader::connect(sequencer, account)?)
    }

    pub fn with_reader(reader: ConvoStateReader) -> Result<Self, ConvoStateReadError> {
        let runtime =
            new_runtime("mls-consensus-blocking").map_err(ConvoStateReadError::Runtime)?;

        Ok(Self {
            reader,
            runtime: Arc::new(runtime),
        })
    }

    /// Shares an existing runtime instead of building a second one.
    const fn from_parts(reader: ConvoStateReader, runtime: Arc<Runtime>) -> Self {
        Self { reader, runtime }
    }

    /// The account this reader follows.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.reader.account()
    }

    pub fn get(&self) -> Result<Option<ConvoState>, ConvoStateReadError> {
        let reader = self.reader.clone();
        self.block_on(async move { reader.get().await })
    }

    pub fn version(&self) -> Result<Option<u64>, ConvoStateReadError> {
        let reader = self.reader.clone();
        self.block_on(async move { reader.version().await })
    }

    pub fn head(&self) -> Result<Option<EpochFingerprint>, ConvoStateReadError> {
        let reader = self.reader.clone();
        self.block_on(async move { reader.head().await })
    }

    #[must_use]
    pub const fn inner(&self) -> &ConvoStateReader {
        &self.reader
    }

    fn block_on<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static) -> T {
        block_on(&self.runtime, future)
    }
}

impl std::fmt::Debug for BlockingReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingReader")
            .field("account", &self.reader.account())
            .finish_non_exhaustive()
    }
}

/// A synchronous [`ConvoStateWriter`]: holds the group's key, and therefore commits.
///
/// Cheap to clone, but note what is shared: clones share one runtime *and one writer*, which
/// means one group-management key and one group. Cloning gives another handle onto the same
/// group, not a second one.
#[derive(Clone)]
pub struct BlockingWriter {
    writer: Arc<ConvoStateWriter>,
    runtime: Arc<Runtime>,
}

impl BlockingWriter {
    /// Connects to a sequencer and derives a group-management key.
    ///
    /// Unlike [`BlockingReader::connect`], this performs I/O: the wallet calibrates against the
    /// sequencer to pick a leader, so it fails if nothing is listening.
    pub fn connect(sequencer: &str) -> Result<Self, ConvoStateWriteError> {
        let runtime =
            new_runtime("mls-consensus-blocking-writer").map_err(ConvoStateReadError::Runtime)?;

        let sequencer = sequencer.to_owned();
        let writer = block_on(
            &runtime,
            async move { ConvoStateWriter::new(&sequencer).await },
        )?;

        Ok(Self {
            writer: Arc::new(writer),
            runtime: Arc::new(runtime),
        })
    }

    /// Wraps a writer built elsewhere — when the caller already has a runtime to build it in.
    pub fn with_writer(writer: ConvoStateWriter) -> Result<Self, ConvoStateWriteError> {
        let runtime =
            new_runtime("mls-consensus-blocking-writer").map_err(ConvoStateReadError::Runtime)?;

        Ok(Self {
            writer: Arc::new(writer),
            runtime: Arc::new(runtime),
        })
    }

    /// The account this writer's group lives in.
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.writer.account()
    }

    /// Deploys the program. Needed once per zone per version of the program.
    pub fn deploy(&self) -> Result<TxHash, ConvoStateWriteError> {
        let writer = Arc::clone(&self.writer);
        self.block_on(move || async move { writer.deploy().await })
    }

    /// Creates the group, claiming an uninitialized account for the program.
    pub fn initialize(&self) -> Result<TxHash, ConvoStateWriteError> {
        let writer = Arc::clone(&self.writer);
        self.block_on(move || async move { writer.initialize().await })
    }

    /// Records the epoch a commit lands the group in, advancing it to `expected_version + 1`.
    ///
    /// Fails with [`ConvoStateWriteError::VersionConflict`] if another member committed first.
    pub fn commit(
        &self,
        expected_version: u64,
        epoch_fingerprint: EpochFingerprint,
    ) -> Result<(), ConvoStateWriteError> {
        let writer = Arc::clone(&self.writer);
        self.block_on(
            move || async move { writer.commit(expected_version, epoch_fingerprint).await },
        )
    }

    /// A keyless blocking reader on the same sequencer, sharing this writer's runtime.
    pub fn reader(&self) -> Result<BlockingReader, ConvoStateReadError> {
        Ok(BlockingReader::from_parts(
            self.writer.reader()?,
            Arc::clone(&self.runtime),
        ))
    }

    #[must_use]
    pub fn inner(&self) -> &ConvoStateWriter {
        &self.writer
    }

    fn block_on<F, Fut, T>(&self, make: F) -> T
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        block_on_local(&self.runtime, make)
    }
}

impl std::fmt::Debug for BlockingWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockingWriter")
            .field("account", &self.writer.account())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pointed at a port nothing is listening on: what is under test is where a call runs, not
    /// what it says.
    fn unreachable_reader() -> BlockingReader {
        BlockingReader::connect("http://127.0.0.1:1", some_account())
            .expect("connect performs no I/O")
    }

    fn some_account() -> AccountId {
        AccountId::new([0_u8; 32])
    }

    #[test]
    fn a_call_from_synchronous_code_returns_rather_than_panics() {
        assert!(unreachable_reader().version().is_err());
    }

    /// The property the channel hand-off exists for: `Runtime::block_on` panics with "Cannot
    /// start a runtime from within a runtime" here.
    #[tokio::test]
    async fn a_call_from_inside_a_runtime_does_not_panic() {
        // On its own thread, because blocking inside a current-thread runtime would stall the
        // very executor the call is waiting on.
        let result = tokio::task::spawn_blocking(|| unreachable_reader().version())
            .await
            .expect("the blocking call panicked inside a runtime");

        assert!(result.is_err());
    }

    #[test]
    fn clones_share_one_runtime() {
        let reader = unreachable_reader();
        let clone = reader.clone();

        assert!(Arc::ptr_eq(&reader.runtime, &clone.runtime));
    }

    /// The writer half. Its operations need a sequencer to reach, so what is exercised here is
    /// the hand-off itself: where the work runs, and that a call returns instead of panicking.
    mod writer {
        use super::*;

        /// Nothing is listening, so the wallet fails to pick a leader — in ~30ms, not by hanging.
        const DEAD_SEQUENCER: &str = "http://127.0.0.1:1";

        fn a_runtime() -> Runtime {
            new_runtime("mls-consensus-blocking-test").expect("a runtime")
        }

        #[test]
        fn connecting_to_a_dead_sequencer_returns_rather_than_panics() {
            assert!(BlockingWriter::connect(DEAD_SEQUENCER).is_err());
        }

        #[tokio::test]
        async fn connecting_from_inside_a_runtime_does_not_panic() {
            let result = tokio::task::spawn_blocking(|| BlockingWriter::connect(DEAD_SEQUENCER))
                .await
                .expect("the blocking call panicked inside a runtime");

            assert!(result.is_err());
        }

        /// What [`block_on_local`] buys over [`block_on`]: the future is never required to be
        /// `Send`, which the writer's futures cannot prove.
        #[test]
        fn a_call_whose_future_is_not_send_returns_rather_than_panics() {
            let value = block_on_local(&a_runtime(), || async {
                // Held across an await, which is what makes the future itself `!Send`. Only the
                // closure has to be `Send`, and it is.
                let not_send = std::rc::Rc::new(7_u8);
                tokio::task::yield_now().await;
                *not_send
            });

            assert_eq!(value, 7);
        }

        /// `Handle::block_on` panics in an async context. A blocking-pool thread is not one —
        /// this is the assumption the whole hand-off rests on.
        #[tokio::test]
        async fn a_call_from_inside_a_runtime_does_not_panic() {
            let value =
                tokio::task::spawn_blocking(|| block_on_local(&a_runtime(), || async { 7_u8 }))
                    .await
                    .expect("the blocking call panicked inside a runtime");

            assert_eq!(value, 7);
        }

        /// The blocking thread parks on the future while the runtime's own worker drives the
        /// timer. If it did not, this would hang rather than fail.
        #[test]
        fn the_runtime_keeps_driving_timers_for_the_parked_thread() {
            block_on_local(&a_runtime(), || async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            });
        }
    }
}
