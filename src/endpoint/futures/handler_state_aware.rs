use crate::endpoint::{ContextInternal, Error};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;
use tracing::warn;

pin_project! {
    /// Future that will stop polling when handler is suspended/failed
    pub struct HandlerStateAwareFuture<F> {
        #[pin]
        fut: F,
        handler_state_rx: oneshot::Receiver<Error>,
        handler_context: ContextInternal,
    }
}

impl<F> HandlerStateAwareFuture<F> {
    pub fn new(
        handler_context: ContextInternal,
        handler_state_rx: oneshot::Receiver<Error>,
        fut: F,
    ) -> HandlerStateAwareFuture<F> {
        HandlerStateAwareFuture {
            fut,
            handler_state_rx,
            handler_context,
        }
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
            Ok(e) => {
                warn!(
                    rpc.system = "restate",
                    rpc.service = %this.handler_context.service_name(),
                    rpc.method = %this.handler_context.handler_name(),
                    "Error while processing handler {e:#}"
                );
                this.handler_context.consume_to_end();
                Poll::Ready(Err(e))
            }
            Err(oneshot::error::TryRecvError::Empty) => match this.fut.poll(cx) {
                Poll::Ready(out) => {
                    this.handler_context.consume_to_end();
                    Poll::Ready(Ok(out))
                }
                Poll::Pending => Poll::Pending,
            },
            Err(oneshot::error::TryRecvError::Closed) => {
                panic!("This is unexpected, this future is still being polled although the sender side was dropped. This should not be possible, because the sender is dropped when this future returns Poll:ready().")
            }
        }
    }
}
