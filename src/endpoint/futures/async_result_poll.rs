use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::{BoxError, ErrorInner};
use restate_sdk_shared_core::{
    DoProgressResponse, Error as CoreError, NotificationHandle, TakeOutputResult, Value, VM,
};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

pub(crate) struct VmAsyncResultPollFuture {
    ctx: Arc<Mutex<ContextInternalInner>>,
    state: Option<PollState>,
}

impl VmAsyncResultPollFuture {
    pub fn maybe_new(
        inner: Cow<'_, Arc<Mutex<ContextInternalInner>>>,
        handle: Result<NotificationHandle, CoreError>,
    ) -> Self {
        VmAsyncResultPollFuture {
            ctx: inner.into_owned(),
            state: Some(match handle {
                Ok(handle) => PollState::Init(handle),
                Err(err) => PollState::Failed(ErrorInner::VM(err)),
            }),
        }
    }

    pub fn new(
        inner: Cow<'_, Arc<Mutex<ContextInternalInner>>>,
        handle: NotificationHandle,
    ) -> Self {
        VmAsyncResultPollFuture {
            ctx: inner.into_owned(),
            state: Some(PollState::Init(handle)),
        }
    }
}

enum PollState {
    Init(NotificationHandle),
    PollProgress(NotificationHandle),
    WaitingInput(NotificationHandle),
    Failed(ErrorInner),
}

macro_rules! must_lock {
    ($mutex:expr) => {
        $mutex.try_lock().expect("You're trying to await two futures at the same time and/or trying to perform some operation on the restate context while awaiting a future. This is not supported!")
    };
}

impl Future for VmAsyncResultPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let ctx = &self.ctx;
        let mut inner_lock = must_lock!(ctx);
        let state = &mut self.state;

        loop {
            match state
                .take()
                .expect("Future should not be polled after Poll::Ready")
            {
                PollState::Init(handle) => {
                    // Let's consume some output to begin with
                    let out = inner_lock.vm.take_output();
                    match out {
                        TakeOutputResult::Buffer(b) => {
                            if !inner_lock.write.send(b) {
                                return Poll::Ready(Err(ErrorInner::Suspended));
                            }
                        }
                        TakeOutputResult::EOF => {
                            return Poll::Ready(Err(ErrorInner::UnexpectedOutputClosed))
                        }
                    }

                    // We can now start polling
                    *state = Some(PollState::PollProgress(handle));
                }
                PollState::WaitingInput(handle) => {
                    let read_result = match inner_lock.read.poll_recv(cx) {
                        Poll::Ready(t) => t,
                        Poll::Pending => {
                            // Still need to wait for input
                            *state = Some(PollState::WaitingInput(handle));
                            return Poll::Pending;
                        }
                    };

                    // Pass read result to VM
                    match read_result {
                        Some(Ok(b)) => inner_lock.vm.notify_input(b),
                        Some(Err(e)) => inner_lock.vm.notify_error(
                            CoreError::new(500u16, format!("Error when reading the body {e:?}",)),
                            None,
                        ),
                        None => inner_lock.vm.notify_input_closed(),
                    }

                    // It's time to poll progress again
                    *state = Some(PollState::PollProgress(handle));
                }
                PollState::PollProgress(handle) => {
                    match inner_lock.vm.do_progress(vec![handle]) {
                        Ok(DoProgressResponse::AnyCompleted) => {
                            // We're good, we got the response
                        }
                        Ok(DoProgressResponse::ReadFromInput) => {
                            *state = Some(PollState::WaitingInput(handle));
                            continue;
                        }
                        Ok(DoProgressResponse::ExecuteRun(_)) => {
                            unimplemented!()
                        }
                        Ok(DoProgressResponse::WaitingPendingRun) => {
                            unimplemented!()
                        }
                        Ok(DoProgressResponse::CancelSignalReceived) => {
                            unimplemented!()
                        }
                        Err(e) => {
                            return Poll::Ready(Err(e.into()));
                        }
                    };

                    // At this point let's try to take the notification
                    match inner_lock.vm.take_notification(handle) {
                        Ok(Some(v)) => return Poll::Ready(Ok(v)),
                        Ok(None) => {
                            panic!(
                                "This is not supposed to happen, handle was flagged as completed"
                            )
                        }
                        Err(e) => return Poll::Ready(Err(e.into())),
                    }
                }
                PollState::Failed(err) => return Poll::Ready(Err(err)),
            }
        }
    }
}
