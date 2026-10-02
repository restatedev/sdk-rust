use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::futures::progress::poll_progress;
use restate_sdk_shared_core::{
    AwaitResponse, NotificationHandle, TerminalFailure, UnresolvedFuture, VM, Value,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Poll, ready};

pub(crate) struct VmAsyncResultPollFuture {
    ctx: Arc<Mutex<ContextInternalInner>>,
    handle: NotificationHandle,
}

impl VmAsyncResultPollFuture {
    pub fn new(ctx: Arc<Mutex<ContextInternalInner>>, handle: NotificationHandle) -> Self {
        Self { ctx, handle }
    }
}

impl Future for VmAsyncResultPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match ready!(poll_progress(
            &self.ctx,
            cx,
            UnresolvedFuture::Single(self.handle)
        ))? {
            AwaitResponse::AnyCompleted => {
                let mut inner = self
                    .ctx
                    .try_lock()
                    .expect("Concurrent access to the Restate context");
                Poll::Ready(Ok(inner
                    .vm
                    .take_notification(self.handle)?
                    .expect("Completed handle has a notification")))
            }
            AwaitResponse::CancelSignalReceived => {
                Poll::Ready(Ok(Value::Failure(TerminalFailure {
                    code: 409,
                    message: "cancelled".to_string(),
                    metadata: vec![],
                })))
            }
            AwaitResponse::ExecuteRun(_) | AwaitResponse::WaitingExternalProgress { .. } => {
                unreachable!("Progress driver resolves execution and waiting internally")
            }
        }
    }
}
