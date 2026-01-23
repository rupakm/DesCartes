use des_tokio::sync::mpsc;
use tonic::Status;

/// A simulation-friendly streaming receiver.
///
/// This is the primary stream type that will appear in public client/server APIs
/// for streaming RPCs.
#[derive(Debug)]
pub struct DesStreaming<T> {
    inner: mpsc::Receiver<Result<T, Status>>,
}

impl<T> DesStreaming<T> {
    pub fn new(inner: mpsc::Receiver<Result<T, Status>>) -> Self {
        Self { inner }
    }

    /// Receive the next item.
    ///
    /// Returns `None` when all senders have been dropped.
    pub async fn next(&mut self) -> Option<Result<T, Status>> {
        self.inner.recv().await
    }

    pub fn into_inner(self) -> mpsc::Receiver<Result<T, Status>> {
        self.inner
    }
}

impl<T> From<mpsc::Receiver<Result<T, Status>>> for DesStreaming<T> {
    fn from(inner: mpsc::Receiver<Result<T, Status>>) -> Self {
        Self::new(inner)
    }
}

/// A simulation-friendly streaming sender.
///
/// This is used for user code to push outbound stream items; it intentionally
/// does not depend on OS-tokio types.
#[derive(Debug, Clone)]
pub struct DesStreamSender<T> {
    inner: mpsc::Sender<T>,
}

impl<T> DesStreamSender<T> {
    pub fn new(inner: mpsc::Sender<T>) -> Self {
        Self { inner }
    }

    pub async fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        self.inner.send(value).await
    }

    /// Close this sender by dropping it.
    ///
    /// Note: the underlying channel is only closed once all sender clones are
    /// dropped.
    pub fn close(self) {
        drop(self);
    }

    pub fn into_inner(self) -> mpsc::Sender<T> {
        self.inner
    }
}

impl<T> From<mpsc::Sender<T>> for DesStreamSender<T> {
    fn from(inner: mpsc::Sender<T>) -> Self {
        Self::new(inner)
    }
}
