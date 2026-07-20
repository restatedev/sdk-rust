use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use crate::errors::TerminalError;
use restate_sdk_shared_core::{
    AwaitResponse, Error as CoreError, NotificationHandle, TerminalFailure, UnresolvedFuture, VM,
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

                    // Let's consume some output to begin with.
                    // Skip empty buffers: take_output returns an empty buffer when there's
                    // nothing to send (e.g. while replaying completed entries). Sending it
                    // would emit an empty HTTP/2 DATA frame per replayed await, which some
                    // proxies (e.g. Envoy) reject when consecutive. See sdk-rust#114.
                    let b = inner_lock.vm.take_output();
                    if !b.is_empty() && !inner_lock.write.send(b) {
                        return Poll::Ready(Err(ErrorInner::Suspended));
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

                    let unresolved_future = UnresolvedFuture::FirstCompleted(
                        handles
                            .iter()
                            .copied()
                            .map(UnresolvedFuture::Single)
                            .collect(),
                    );
                    match inner_lock.vm.do_await(unresolved_future) {
                        Ok(AwaitResponse::AnyCompleted) => {
                            // We're good, we got the response
                        }
                        Ok(AwaitResponse::WaitingExternalProgress {
                            waiting_input: true,
                            ..
                        }) => {
                            // do_await buffered an AwaitingOnMessage describing what we're
                            // suspending on; flush it to the runtime before we block on input,
                            // otherwise the runtime never learns what we're waiting for.
                            let b = inner_lock.vm.take_output();
                            if !b.is_empty() && !inner_lock.write.send(b) {
                                return Poll::Ready(Err(ErrorInner::Suspended));
                            }
                            drop(inner_lock);
                            self.state =
                                Some(VmSelectAsyncResultPollState::WaitingInput { ctx, handles });
                            continue;
                        }
                        Ok(AwaitResponse::WaitingExternalProgress { .. })
                        | Ok(AwaitResponse::ExecuteRun(_)) => {
                            // Waiting only on a run proposal is not expected on this await path.
                            // These two are used by the shared-core to implement async run, which is not supported by the rust sdk currently.
                            unimplemented!()
                        }
                        Ok(AwaitResponse::CancelSignalReceived) => {
                            return Poll::Ready(Ok(Err(TerminalFailure {
                                code: 409,
                                message: "cancelled".to_string(),
                                metadata: vec![],
                            }
                            .into())));
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
