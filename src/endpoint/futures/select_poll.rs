use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::futures::progress::{ProgressResult, ProgressWaiter, poll_progress};
use crate::errors::TerminalError;
use restate_sdk_shared_core::{NotificationHandle, TerminalFailure, UnresolvedFuture, VM};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Poll, ready};

pub(crate) struct VmSelectAsyncResultPollFuture {
    ctx: Arc<Mutex<ContextInternalInner>>,
    handles: Vec<NotificationHandle>,
    waiter: ProgressWaiter,
}

impl VmSelectAsyncResultPollFuture {
    pub fn new(
        ctx: Arc<Mutex<ContextInternalInner>>,
        handles: Vec<NotificationHandle>,
        generation: usize,
    ) -> Self {
        Self {
            ctx,
            handles,
            waiter: ProgressWaiter::new(generation),
        }
    }
}

impl Future for VmSelectAsyncResultPollFuture {
    type Output = Result<Result<usize, TerminalError>, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let unresolved = UnresolvedFuture::FirstCompleted(
            self.handles
                .iter()
                .copied()
                .map(UnresolvedFuture::Single)
                .collect(),
        );
        let this = self.as_mut().get_mut();
        match ready!(poll_progress(
            &this.ctx,
            cx,
            &mut this.waiter,
            unresolved,
            |inner| {
                Ok(this
                    .handles
                    .iter()
                    .position(|handle| inner.vm.is_completed(*handle))
                    .expect("Completed selection has a ready handle"))
            },
        ))? {
            ProgressResult::Completed(index) => Poll::Ready(Ok(Ok(index))),
            ProgressResult::Cancelled => Poll::Ready(Ok(Err(TerminalFailure {
                code: 409,
                message: "cancelled".to_string(),
                metadata: vec![],
            }
            .into()))),
        }
    }
}
