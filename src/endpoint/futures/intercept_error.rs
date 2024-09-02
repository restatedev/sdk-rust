use crate::context::{RunFuture, RunRetryPolicy};
use crate::endpoint::{ContextInternal, Error};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

pin_project! {
    /// Future that intercepts errors of inner future, and passes them to ContextInternal
    pub struct InterceptErrorFuture<F>{
        #[pin]
        fut: F,
        ctx: ContextInternal
    }
}

impl<F> InterceptErrorFuture<F> {
    pub fn new(ctx: ContextInternal, fut: F) -> Self {
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

impl<F, R> RunFuture<R> for InterceptErrorFuture<F>
where
    F: RunFuture<Result<R, Error>>,
{
    fn retry_policy(mut self, retry_policy: RunRetryPolicy) -> Self {
        self.fut = self.fut.retry_policy(retry_policy);
        self
    }

    fn name(mut self, name: impl Into<String>) -> Self {
        self.fut = self.fut.name(name);
        self
    }
}
