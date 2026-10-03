use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::futures::progress::{ProgressResult, ProgressWaiter, poll_progress};
use restate_sdk_shared_core::{NotificationHandle, TerminalFailure, UnresolvedFuture, VM, Value};
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
    ) -> Self {
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
    inner: &mut ContextInternalInner,
    handle: NotificationHandle,
) -> Result<Value, ErrorInner> {
    let notification = inner
        .vm
        .take_notification(handle)?
        .expect("Completed handle has a notification");
    inner.notifications.remove(&handle);
    Ok(notification)
}

impl Future for VmAsyncResultPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        match ready!(poll_progress(
            &this.ctx,
            cx,
            &mut this.waiter,
            UnresolvedFuture::Single(this.handle),
            |inner| take_completed_result(inner, this.handle),
        ))? {
            ProgressResult::Completed(notification) => Poll::Ready(Ok(notification)),
            ProgressResult::Cancelled => Poll::Ready(Ok(Value::Failure(TerminalFailure {
                code: 409,
                message: "cancelled".to_string(),
                metadata: vec![],
            }))),
        }
    }
}
