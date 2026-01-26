use des_tokio::sync::mpsc;
use std::sync::{Arc, Mutex};
use tonic::Status;

type TypedSender<T> = Arc<Mutex<Option<mpsc::Sender<Result<T, Status>>>>>;

pub fn channel<T>(cap: usize) -> (Sender<T>, DesStreaming<T>) {
    let (tx, rx) = mpsc::channel::<Result<T, Status>>(cap);
    (
        Sender {
            inner: Arc::new(Mutex::new(Some(tx))),
        },
        DesStreaming::new(rx),
    )
}

/// Typed sender for streaming RPCs.
///
/// This wraps an internal `mpsc::Sender<Result<T, Status>>` so callers can send
/// either normal stream items or an explicit error.
#[derive(Debug, Clone)]
pub struct Sender<T> {
    inner: TypedSender<T>,
}

impl<T> Sender<T> {
    pub async fn send(&self, item: T) -> Result<(), ()> {
        let tx = {
            let guard = self.inner.lock().unwrap();
            guard.as_ref().cloned()
        };

        let Some(tx) = tx else {
            return Err(());
        };

        tx.send(Ok(item)).await.map_err(|_| ())
    }

    pub async fn send_err(&self, status: Status) -> Result<(), ()> {
        let tx = {
            let guard = self.inner.lock().unwrap();
            guard.as_ref().cloned()
        };

        let Some(tx) = tx else {
            return Err(());
        };

        tx.send(Err(status)).await.map_err(|_| ())
    }

    pub fn close(&self) {
        let _ = self.inner.lock().unwrap().take();
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use des_core::{Execute, Executor, SimTime, Simulation, SimulationConfig};
    use tonic::Code;

    #[test]
    fn typed_stream_channel_sends_ok_and_err() {
        let mut sim = Simulation::new(SimulationConfig { seed: 1 });
        des_tokio::runtime::install(&mut sim);

        let out: std::sync::Arc<std::sync::Mutex<Vec<Result<u32, Code>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let out2 = out.clone();

        des_tokio::task::spawn_local(async move {
            let (tx, mut rx) = channel::<u32>(4);
            tx.send(7).await.unwrap();
            tx.send_err(Status::new(Code::InvalidArgument, "nope"))
                .await
                .unwrap();
            tx.close();

            if let Some(item) = rx.next().await {
                out2.lock().unwrap().push(item.map_err(|s| s.code()));
            }
            if let Some(item) = rx.next().await {
                out2.lock().unwrap().push(item.map_err(|s| s.code()));
            }
            if let Some(item) = rx.next().await {
                out2.lock().unwrap().push(item.map_err(|s| s.code()));
            }
        });

        Executor::timed(SimTime::from_millis(1)).execute(&mut sim);

        assert_eq!(
            *out.lock().unwrap(),
            vec![Ok(7), Err(Code::InvalidArgument)]
        );
    }
}
