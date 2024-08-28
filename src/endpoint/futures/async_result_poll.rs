use crate::endpoint::context::ContextInternalInner;
use crate::endpoint::ErrorInner;
use restate_sdk_shared_core::{
    AsyncResultHandle, SuspendedOrVMError, TakeOutputResult, VMError, Value, VM,
};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

pub(crate) struct VmAsyncResultPollFuture {
    state: Option<PollState>,
}

impl VmAsyncResultPollFuture {
    pub fn new(
        inner: Cow<'_, Arc<Mutex<ContextInternalInner>>>,
        handle: Result<AsyncResultHandle, VMError>,
    ) -> Self {
        VmAsyncResultPollFuture {
            state: Some(match handle {
                Ok(handle) => PollState::Init {
                    ctx: inner.into_owned(),
                    handle,
                },
                Err(err) => PollState::Failed(ErrorInner::VM(err)),
            }),
        }
    }
}

enum PollState {
    Init {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: AsyncResultHandle,
    },
    WaitingInput {
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: AsyncResultHandle,
    },
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
        loop {
            match self
                .state
                .take()
                .expect("Future should not be polled after Poll::Ready")
            {
                PollState::Init { ctx, handle } => {
                    // Acquire lock
                    let mut inner_lock = must_lock!(ctx);

                    // Let's consume some output
                    let out = inner_lock.vm.take_output();
                    match out {
                        TakeOutputResult::Buffer(b) => {
                            if !inner_lock.write.send(b) {
                                self.state = Some(PollState::Failed(ErrorInner::Suspended));
                                continue;
                            }
                        }
                        TakeOutputResult::EOF => {
                            self.state =
                                Some(PollState::Failed(ErrorInner::UnexpectedOutputClosed));
                            continue;
                        }
                    }

                    // Notify that we reached an await point
                    inner_lock.vm.notify_await_point(handle);

                    // At this point let's try to take the async result
                    match inner_lock.vm.take_async_result(handle) {
                        Ok(Some(v)) => return Poll::Ready(Ok(v)),
                        Ok(None) => {
                            drop(inner_lock);
                            self.state = Some(PollState::WaitingInput { ctx, handle });
                        }
                        Err(SuspendedOrVMError::Suspended(_)) => {
                            self.state = Some(PollState::Failed(ErrorInner::Suspended));
                        }
                        Err(SuspendedOrVMError::VM(e)) => {
                            self.state = Some(PollState::Failed(ErrorInner::VM(e)));
                        }
                    }
                }
                PollState::WaitingInput { ctx, handle } => {
                    let mut inner_lock = must_lock!(ctx);

                    let read_result = match inner_lock.read.poll_recv(cx) {
                        Poll::Ready(t) => t,
                        Poll::Pending => {
                            drop(inner_lock);
                            self.state = Some(PollState::WaitingInput { ctx, handle });
                            return Poll::Pending;
                        }
                    };

                    // Pass read result to VM
                    match read_result {
                        Some(Ok(b)) => inner_lock.vm.notify_input(b),
                        Some(Err(e)) => inner_lock.vm.notify_error(
                            "Error when reading the body".into(),
                            e.to_string().into(),
                            None,
                        ),
                        None => inner_lock.vm.notify_input_closed(),
                    }

                    // Now try to take async result again
                    match inner_lock.vm.take_async_result(handle) {
                        Ok(Some(v)) => return Poll::Ready(Ok(v)),
                        Ok(None) => {
                            drop(inner_lock);
                            self.state = Some(PollState::WaitingInput { ctx, handle });
                        }
                        Err(SuspendedOrVMError::Suspended(_)) => {
                            self.state = Some(PollState::Failed(ErrorInner::Suspended));
                        }
                        Err(SuspendedOrVMError::VM(e)) => {
                            self.state = Some(PollState::Failed(ErrorInner::VM(e)));
                        }
                    }
                }
                PollState::Failed(err) => return Poll::Ready(Err(err)),
            }
        }
    }
}
