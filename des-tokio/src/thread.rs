use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::task::JoinHandle;

/// Spawn a task as a simulated "thread".
///
/// This is a thin wrapper around [`crate::task::spawn`], intended to provide a
/// `std::thread`-like API surface while remaining compatible with the DES async
/// runtime.
pub fn spawn<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    crate::task::spawn(future)
}

/// Yield execution to allow another ready task to run.
///
/// This is useful for race exploration: explicit yield points allow users to
/// model cooperative preemption.
pub async fn yield_now() {
    struct YieldNow {
        yielded: bool,
    }

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    YieldNow { yielded: false }.await
}
