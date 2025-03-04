use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::ErrorInner;
use restate_sdk_shared_core::{
    DoProgressResponse, Error as CoreError, NotificationHandle, TakeOutputResult, Value, VM,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

pub(crate) struct VmAsyncResultPollFuture {
    state: Option<AsyncResultPollState>,
}

impl VmAsyncResultPollFuture {
    pub fn new(ctx: Arc<Mutex<ContextInternalInner>>, handle: NotificationHandle) -> Self {
        VmAsyncResultPollFuture {
            state: Some(AsyncResultPollState::Init { ctx, handle }),
        }
    }
}

enum AsyncResultPollState {
    Init {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: NotificationHandle,
    },
    PollProgress {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: NotificationHandle,
    },
    WaitingInput {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: NotificationHandle,
    },
}

macro_rules! must_lock {
    ($mutex:expr) => {
        $mutex.try_lock().expect("You're trying to await two futures at the same time and/or trying to perform some operation on the restate context while awaiting a future. This is not supported!")
    };
}

impl Future for VmAsyncResultPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        loop {
            match self
                .state
                .take()
                .expect("Future should not be polled after Poll::Ready")
            {
                AsyncResultPollState::Init { ctx, handle } => {
                    let mut inner_lock = must_lock!(ctx);

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
                    drop(inner_lock);
                    self.state = Some(AsyncResultPollState::PollProgress { ctx, handle });
                }
                AsyncResultPollState::WaitingInput { ctx, handle } => {
                    let mut inner_lock = must_lock!(ctx);

                    let read_result = match inner_lock.read.poll_recv(cx) {
                        Poll::Ready(t) => t,
                        Poll::Pending => {
                            // Still need to wait for input
                            drop(inner_lock);
                            self.state = Some(AsyncResultPollState::WaitingInput { ctx, handle });
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
                    drop(inner_lock);
                    self.state = Some(AsyncResultPollState::PollProgress { ctx, handle });
                }
                AsyncResultPollState::PollProgress { ctx, handle } => {
                    let mut inner_lock = must_lock!(ctx);

                    match inner_lock.vm.do_progress(vec![handle]) {
                        Ok(DoProgressResponse::AnyCompleted) => {
                            // We're good, we got the response
                        }
                        Ok(DoProgressResponse::ReadFromInput) => {
                            drop(inner_lock);
                            self.state = Some(AsyncResultPollState::WaitingInput { ctx, handle });
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
            }
        }
    }
}
