use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::ErrorInner;
use crate::errors::TerminalError;
use restate_sdk_shared_core::{
    DoProgressResponse, Error as CoreError, NotificationHandle, TakeOutputResult, TerminalFailure,
    VM,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

pub(crate) struct VmSelectAsyncResultPollFuture {
    state: Option<VmSelectAsyncResultPollState>,
}

impl VmSelectAsyncResultPollFuture {
    pub fn new(ctx: Arc<Mutex<ContextInternalInner>>, handles: Vec<NotificationHandle>) -> Self {
        VmSelectAsyncResultPollFuture {
            state: Some(VmSelectAsyncResultPollState::Init { ctx, handles }),
        }
    }
}

enum VmSelectAsyncResultPollState {
    Init {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handles: Vec<NotificationHandle>,
    },
    PollProgress {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handles: Vec<NotificationHandle>,
    },
    WaitingInput {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handles: Vec<NotificationHandle>,
    },
}

macro_rules! must_lock {
    ($mutex:expr) => {
        $mutex.try_lock().expect("You're trying to await two futures at the same time and/or trying to perform some operation on the restate context while awaiting a future. This is not supported!")
    };
}

impl Future for VmSelectAsyncResultPollFuture {
    type Output = Result<Result<usize, TerminalError>, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        loop {
            match self
                .state
                .take()
                .expect("Future should not be polled after Poll::Ready")
            {
                VmSelectAsyncResultPollState::Init { ctx, handles } => {
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
                    self.state = Some(VmSelectAsyncResultPollState::PollProgress { ctx, handles });
                }
                VmSelectAsyncResultPollState::WaitingInput { ctx, handles } => {
                    let mut inner_lock = must_lock!(ctx);

                    let read_result = match inner_lock.read.poll_recv(cx) {
                        Poll::Ready(t) => t,
                        Poll::Pending => {
                            // Still need to wait for input
                            drop(inner_lock);
                            self.state =
                                Some(VmSelectAsyncResultPollState::WaitingInput { ctx, handles });
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
                    self.state = Some(VmSelectAsyncResultPollState::PollProgress { ctx, handles });
                }
                VmSelectAsyncResultPollState::PollProgress { ctx, handles } => {
                    let mut inner_lock = must_lock!(ctx);

                    match inner_lock.vm.do_progress(handles.clone()) {
                        Ok(DoProgressResponse::AnyCompleted) => {
                            // We're good, we got the response
                        }
                        Ok(DoProgressResponse::ReadFromInput) => {
                            drop(inner_lock);
                            self.state =
                                Some(VmSelectAsyncResultPollState::WaitingInput { ctx, handles });
                            continue;
                        }
                        Ok(DoProgressResponse::ExecuteRun(_)) => {
                            unimplemented!()
                        }
                        Ok(DoProgressResponse::WaitingPendingRun) => {
                            unimplemented!()
                        }
                        Ok(DoProgressResponse::CancelSignalReceived) => {
                            return Poll::Ready(Ok(Err(TerminalFailure {
                                code: 409,
                                message: "cancelled".to_string(),
                            }
                            .into())))
                        }
                        Err(e) => {
                            return Poll::Ready(Err(e.into()));
                        }
                    };

                    // DoProgress might cause a flip of the replaying state
                    inner_lock.maybe_flip_span_replaying_field();

                    // At this point let's try to take the notification
                    for (idx, handle) in handles.iter().enumerate() {
                        if inner_lock.vm.is_completed(*handle) {
                            return Poll::Ready(Ok(Ok(idx)));
                        }
                    }
                    panic!(
                        "This is not supposed to happen, none of the given handles were completed even though poll progress completed with AnyCompleted"
                    )
                }
            }
        }
    }
}
