use dispatch2::{DispatchQueue, DispatchRetained};
use tokio::sync::oneshot;

use crate::KasouError;

/// Wrapper around a serial dispatch queue for VM operations.
///
/// All `VZVirtualMachine` operations must happen on a serial dispatch queue.
/// This struct bridges dispatch queue execution to tokio's async model
/// via oneshot channels.
///
/// Currently unused — VmHandle uses the main queue via `initWithConfiguration:`.
/// Retained for future use with `initWithConfiguration:queue:`.
#[allow(dead_code)]
pub(crate) struct VmQueue {
    queue: DispatchRetained<DispatchQueue>,
}

#[allow(dead_code)]
impl VmQueue {
    /// Create a new serial dispatch queue for VM operations.
    pub fn new(label: &str) -> Self {
        let queue = DispatchQueue::new(label, None);
        Self { queue }
    }

    /// Execute a closure on the dispatch queue and await the result from tokio.
    ///
    /// The closure runs on the dispatch queue thread. The result is sent back
    /// to the calling tokio task via a oneshot channel.
    pub async fn run<F, T>(&self, f: F) -> Result<T, KasouError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.queue.exec_async(move || {
            let result = f();
            let _ = tx.send(result);
        });
        rx.await.map_err(|_| KasouError::QueueCancelled)
    }

    /// Get a reference to the underlying dispatch queue.
    ///
    /// Used when creating `VZVirtualMachine` with `initWithConfiguration:queue:`.
    pub fn inner(&self) -> &DispatchQueue {
        &self.queue
    }
}
