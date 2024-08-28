use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Future that traps the execution at this point, but keeps waking up the waker
pub struct TrapFuture<T>(PhantomData<fn() -> T>);

impl<T> Default for TrapFuture<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T> Future for TrapFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<T> {
        ctx.waker().wake_by_ref();
        Poll::Pending
    }
}
