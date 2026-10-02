use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::futures::progress::poll_progress;
use crate::errors::TerminalError;
use restate_sdk_shared_core::{
    AwaitResponse, NotificationHandle, TerminalFailure, UnresolvedFuture, VM,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Poll, ready};

pub(crate) struct VmSelectAsyncResultPollFuture {
    ctx: Arc<Mutex<ContextInternalInner>>,
    handles: Vec<NotificationHandle>,
}

impl VmSelectAsyncResultPollFuture {
    pub fn new(ctx: Arc<Mutex<ContextInternalInner>>, handles: Vec<NotificationHandle>) -> Self {
        Self { ctx, handles }
    }
}

impl Future for VmSelectAsyncResultPollFuture {
    type Output = Result<Result<usize, TerminalError>, ErrorInner>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let unresolved = UnresolvedFuture::FirstCompleted(
            self.handles
                .iter()
                .copied()
                .map(UnresolvedFuture::Single)
                .collect(),
        );
        match ready!(poll_progress(&self.ctx, cx, unresolved))? {
            AwaitResponse::AnyCompleted => {
                let inner = self
                    .ctx
                    .try_lock()
                    .expect("Concurrent access to the Restate context");
                Poll::Ready(Ok(Ok(self
                    .handles
                    .iter()
                    .position(|handle| inner.vm.is_completed(*handle))
                    .expect("Completed selection has a ready handle"))))
            }
            AwaitResponse::CancelSignalReceived => Poll::Ready(Ok(Err(TerminalFailure {
                code: 409,
                message: "cancelled".to_string(),
                metadata: vec![],
            }
            .into()))),
            AwaitResponse::ExecuteRun(_) | AwaitResponse::WaitingExternalProgress { .. } => {
                unreachable!("Progress driver resolves execution and waiting internally")
            }
        }
    }
}
