use crate::context::DurableFuture;
use crate::endpoint::{ContextInternal, Error};
use pin_project_lite::pin_project;
use restate_sdk_shared_core::NotificationHandle;
use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

pin_project! {
    /// Future that intercepts errors of inner future, and passes them to ContextInternal
    pub struct DurableFutureImpl<F>{
        #[pin]
        fut: F,
        handle: NotificationHandle,
        ctx: ContextInternal
    }
}

impl<F> DurableFutureImpl<F> {
    pub fn new(ctx: ContextInternal, handle: NotificationHandle, fut: F) -> Self {
        Self { fut, handle, ctx }
    }
}

impl<F, R> Future for DurableFutureImpl<F>
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

impl<F> crate::context::macro_support::SealedDurableFuture for DurableFutureImpl<F> {
    fn inner_context(&self) -> ContextInternal {
        self.ctx.clone()
    }

    fn handle(&self) -> NotificationHandle {
        self.handle
    }
}

impl<F, R> DurableFuture for DurableFutureImpl<F> where F: Future<Output = Result<R, Error>> {}
