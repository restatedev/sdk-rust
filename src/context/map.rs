use crate::context::DurableFuture;
use crate::context::macro_support::SealedDurableFuture;
use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use pin_project_lite::pin_project;
use restate_sdk_shared_core::NotificationHandle;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

pin_project! {
    /// See [`DurableFuture::map`].
    pub struct MapDurableFuture<F, M> {
        #[pin]
        fut: F,
        mapper: Option<M>,
    }
}

impl<F, M> MapDurableFuture<F, M> {
    pub(crate) fn new(fut: F, mapper: M) -> Self {
        Self {
            fut,
            mapper: Some(mapper),
        }
    }
}

impl<F, M, U> Future for MapDurableFuture<F, M>
where
    F: Future,
    M: FnOnce(F::Output) -> U,
{
    type Output = U;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let output = ready!(this.fut.poll(cx));
        let mapper = this
            .mapper
            .take()
            .expect("MapDurableFuture polled after completion");
        Poll::Ready(mapper(output))
    }
}

impl<F: SealedDurableFuture, M> SealedDurableFuture for MapDurableFuture<F, M> {
    fn inner_context(&self) -> ContextInternal {
        self.fut.inner_context()
    }

    fn handle(&self) -> NotificationHandle {
        self.fut.handle()
    }
}

impl<F, M, U> DurableFuture for MapDurableFuture<F, M>
where
    F: DurableFuture,
    M: FnOnce(F::Output) -> U,
{
}

pin_project! {
    /// See [`DurableFuture::map_ok`].
    pub struct MapOkDurableFuture<F, M> {
        #[pin]
        fut: F,
        mapper: Option<M>,
    }
}

impl<F, M> MapOkDurableFuture<F, M> {
    pub(crate) fn new(fut: F, mapper: M) -> Self {
        Self {
            fut,
            mapper: Some(mapper),
        }
    }
}

impl<F, M, T, U> Future for MapOkDurableFuture<F, M>
where
    F: Future<Output = Result<T, TerminalError>>,
    M: FnOnce(T) -> U,
{
    type Output = Result<U, TerminalError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let output = ready!(this.fut.poll(cx));
        let mapper = this
            .mapper
            .take()
            .expect("MapOkDurableFuture polled after completion");
        Poll::Ready(output.map(mapper))
    }
}

impl<F: SealedDurableFuture, M> SealedDurableFuture for MapOkDurableFuture<F, M> {
    fn inner_context(&self) -> ContextInternal {
        self.fut.inner_context()
    }

    fn handle(&self) -> NotificationHandle {
        self.fut.handle()
    }
}

impl<F, M, T, U> DurableFuture for MapOkDurableFuture<F, M>
where
    F: DurableFuture + Future<Output = Result<T, TerminalError>>,
    M: FnOnce(T) -> U,
{
}

pin_project! {
    /// See [`DurableFuture::map_err`].
    pub struct MapErrDurableFuture<F, M> {
        #[pin]
        fut: F,
        mapper: Option<M>,
    }
}

impl<F, M> MapErrDurableFuture<F, M> {
    pub(crate) fn new(fut: F, mapper: M) -> Self {
        Self {
            fut,
            mapper: Some(mapper),
        }
    }
}

impl<F, M, T> Future for MapErrDurableFuture<F, M>
where
    F: Future<Output = Result<T, TerminalError>>,
    M: FnOnce(TerminalError) -> TerminalError,
{
    type Output = Result<T, TerminalError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let output = ready!(this.fut.poll(cx));
        let mapper = this
            .mapper
            .take()
            .expect("MapErrDurableFuture polled after completion");
        Poll::Ready(output.map_err(mapper))
    }
}

impl<F: SealedDurableFuture, M> SealedDurableFuture for MapErrDurableFuture<F, M> {
    fn inner_context(&self) -> ContextInternal {
        self.fut.inner_context()
    }

    fn handle(&self) -> NotificationHandle {
        self.fut.handle()
    }
}

impl<F, M, T> DurableFuture for MapErrDurableFuture<F, M>
where
    F: DurableFuture + Future<Output = Result<T, TerminalError>>,
    M: FnOnce(TerminalError) -> TerminalError,
{
}
