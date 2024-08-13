use crate::endpoint::{ContextInternal, Error};
use pin_project_lite::pin_project;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;
use tracing::warn;

/// Future that traps the execution at this point, but keeps waking up the waker
pub(super) struct TrapFuture<T>(PhantomData<T>);

impl<T> Default for TrapFuture<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

/// This is always safe, because we simply use phantom data inside TrapFuture.
unsafe impl<T> Send for TrapFuture<T> {}
unsafe impl<T> Sync for TrapFuture<T> {}

impl<T> Future for TrapFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> Poll<T> {
        ctx.waker().wake_by_ref();
        Poll::Pending
    }
}

pin_project! {
    /// Future that intercepts errors of inner future, and passes them to ContextInternal
    pub(super) struct InterceptErrorFuture<F>{
        #[pin]
        fut: F,
        ctx: ContextInternal
    }
}

impl<F> InterceptErrorFuture<F> {
    pub(super) fn new(ctx: ContextInternal, fut: F) -> Self {
        Self { fut, ctx }
    }
}

impl<F, R> Future for InterceptErrorFuture<F>
where
    F: Future<Output = Result<R, Error>>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let result = ready!(this.fut.poll(cx));

        match result {
            Ok(r) => Poll::Ready(r),
            Err(e) => {
                this.ctx.fail(e);

                // Here is the secret sauce. This will immediately cause the whole future chain to be polled,
                //  but the poll here will be intercepted by HandlerStateAwareFuture
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

pin_project! {
    /// Future that will stop polling when handler is suspended/failed
    pub(super) struct HandlerStateAwareFuture<F> {
        #[pin]
        fut: F,
        handler_state_rx: oneshot::Receiver<Error>,
        handler_context: ContextInternal,
    }
}

impl<F> HandlerStateAwareFuture<F> {
    pub(super) fn new(
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
