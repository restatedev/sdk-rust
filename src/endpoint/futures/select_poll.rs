use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextShared;
use crate::endpoint::futures::progress::{Awaited, ProgressGuard, ProgressResult, poll_progress};
use crate::errors::TerminalError;
use restate_sdk_shared_core::{NotificationHandle, TerminalFailure, VM};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Poll, ready};

pub(crate) struct VmSelectAsyncResultPollFuture {
    guard: ProgressGuard,
    handles: Vec<NotificationHandle>,
}

impl VmSelectAsyncResultPollFuture {
    pub fn new(
        ctx: Arc<ContextShared>,
        handles: Vec<NotificationHandle>,
        generation: usize,
    ) -> Self {
        Self {
            guard: ProgressGuard::new(ctx, generation),
            handles,
        }
    }
}

impl Future for VmSelectAsyncResultPollFuture {
    type Output = Result<Result<usize, TerminalError>, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        match ready!(poll_progress(
            &mut this.guard,
            cx,
            Awaited::FirstCompleted(&this.handles),
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
