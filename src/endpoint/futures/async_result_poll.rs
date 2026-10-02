use crate::endpoint::ErrorInner;
use crate::endpoint::context::{CONTEXT_LOCK_ERROR, ContextInternalInner};
use crate::endpoint::futures::progress::{
    CancellationLedger, ProgressWaiter, flush, poll_progress,
};
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
    waiter: ProgressWaiter,
}

impl VmAsyncResultPollFuture {
    pub fn new(
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: NotificationHandle,
        generation: usize,
        notifications: CancellationLedger,
    ) -> Self {
        notifications
            .lock()
            .unwrap()
            .entry(handle)
            .or_insert(generation);
        Self {
            ctx,
            handle,
            waiter: ProgressWaiter::new(generation),
        }
    }
}

/// Known completions win over invocation cancellation, including results that
/// were acknowledged before another waiter consumed the cancellation signal.
fn take_completed_result(
    ctx: &Arc<Mutex<ContextInternalInner>>,
    handle: NotificationHandle,
) -> Result<Option<Value>, ErrorInner> {
    let mut inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
    if !inner.vm.is_completed(handle) {
        return Ok(None);
    }
    flush(&mut inner)?;
    let notification = inner
        .vm
        .take_notification(handle)?
        .expect("Completed handle has a notification");
    inner.notifications.lock().unwrap().remove(&handle);
    Ok(Some(notification))
}

impl Future for VmAsyncResultPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(notification) = take_completed_result(&self.ctx, self.handle)? {
            return Poll::Ready(Ok(notification));
        }
        let this = self.as_mut().get_mut();
        match ready!(poll_progress(
            &this.ctx,
            cx,
            &mut this.waiter,
            UnresolvedFuture::Single(this.handle)
        ))? {
            AwaitResponse::AnyCompleted => {
                Poll::Ready(Ok(take_completed_result(&self.ctx, self.handle)?
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
