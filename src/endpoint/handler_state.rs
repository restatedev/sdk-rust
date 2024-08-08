use crate::endpoint::{Error, ErrorInner};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;

pub(super) struct HandlerStateNotifier {
    tx: Option<oneshot::Sender<Error>>,
}

impl HandlerStateNotifier {
    pub(crate) fn new() -> (Self, oneshot::Receiver<Error>) {
        let (tx, rx) = oneshot::channel();
        return (Self { tx: Some(tx) }, rx);
    }

    pub(super) fn mark_error(&mut self, err: Error) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(err);
        }
    }

    pub(super) fn mark_error_inner(&mut self, err: ErrorInner) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Error(err));
        }
        // Some other operation already marked this handler as errored.
    }

    pub(super) fn mark_suspend(&mut self) {
        self.mark_error_inner(ErrorInner::Suspended)
    }
}

pub(super) fn handler_state_aware_future<F: Future + Send>(
    handler_state_rx: oneshot::Receiver<Error>,
    f: F,
) -> impl Future<Output = Result<F::Output, Error>> + Send {
    HandlerStateAwareFuture {
        fut: f,
        handler_state_rx,
    }
}

pin_project! {
    /// Future that will stop polling when handler is suspended/failed
    struct HandlerStateAwareFuture<F> {
        #[pin]
        fut: F,
        handler_state_rx: oneshot::Receiver<Error>,
    }
}

impl<F> Future for HandlerStateAwareFuture<F>
where
    F: Future,
{
    type Output = Result<F::Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.handler_state_rx.try_recv() {
            Ok(e) => Poll::Ready(Err(e)),
            Err(oneshot::error::TryRecvError::Empty) => this.fut.poll(cx).map(Ok),
            Err(oneshot::error::TryRecvError::Closed) => {
                panic!("This is unexpected, this future is still being polled although the sender side was dropped. This should not be possible, because the sender is dropped when this future returns Poll:ready().")
            }
        }
    }
}
