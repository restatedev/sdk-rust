use crate::endpoint::ErrorInner;
use crate::endpoint::context::ContextInternalInner;
use futures::future::BoxFuture;
use futures::task::{ArcWake, waker_ref};
use restate_sdk_shared_core::{
    AwaitResponse, Error as CoreError, RetryPolicy, RunExitResult, UnresolvedFuture, VM,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// One input stream and all owned run closures share a wakeup source. Input may
/// satisfy a sibling future, so its consumer must wake every registered waiter.
#[derive(Default)]
pub(crate) struct ProgressWakers(Mutex<Vec<Waker>>);

impl ProgressWakers {
    fn register(&self, waker: &Waker) {
        let mut waiters = self.0.lock().unwrap();
        if !waiters.iter().any(|waiter| waiter.will_wake(waker)) {
            waiters.push(waker.clone());
        }
    }
}

impl ArcWake for ProgressWakers {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let waiters = arc_self.0.lock().unwrap().clone();
        for waiter in waiters {
            waiter.wake();
        }
    }
}

pub(crate) struct RegisteredRun {
    pub(crate) future: BoxFuture<'static, Result<RunExitResult, ErrorInner>>,
    pub(crate) retry_policy: RetryPolicy,
    executing: bool,
}

impl RegisteredRun {
    pub(crate) fn new(
        future: BoxFuture<'static, Result<RunExitResult, ErrorInner>>,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            future,
            retry_policy,
            executing: false,
        }
    }
}

fn flush(inner: &mut ContextInternalInner) -> Result<(), ErrorInner> {
    let output = inner.vm.take_output();
    if !output.is_empty() && !inner.write.send(output) {
        return Err(ErrorInner::Suspended);
    }
    Ok(())
}

/// Drive the VM, owned closures, and input together. User futures are never
/// polled with the VM mutex held.
pub(crate) fn poll_progress(
    ctx: &Arc<Mutex<ContextInternalInner>>,
    cx: &mut Context<'_>,
    awaited: UnresolvedFuture,
) -> Poll<Result<AwaitResponse, ErrorInner>> {
    let wakers = {
        let inner = ctx
            .try_lock()
            .expect("Concurrent access to the Restate context");
        Arc::clone(&inner.progress_wakers)
    };
    wakers.register(cx.waker());
    let waker = waker_ref(&wakers);
    let mut shared_cx = Context::from_waker(&waker);

    loop {
        let mut executing = {
            let mut inner = ctx
                .try_lock()
                .expect("Concurrent access to the Restate context");
            let handles: Vec<_> = inner
                .runs
                .iter()
                .filter(|(_, run)| run.executing)
                .map(|(handle, _)| *handle)
                .collect();
            handles
                .into_iter()
                .map(|handle| (handle, inner.runs.remove(&handle).unwrap()))
                .collect::<HashMap<_, _>>()
        };
        for (handle, mut run) in executing.drain() {
            match run.future.as_mut().poll(&mut shared_cx) {
                Poll::Ready(result) => {
                    let mut inner = ctx
                        .try_lock()
                        .expect("Concurrent access to the Restate context");
                    inner
                        .vm
                        .propose_run_completion(handle, result?, run.retry_policy)?;
                    inner.maybe_flip_span_replaying_field();
                    flush(&mut inner)?;
                    drop(inner);
                    ArcWake::wake_by_ref(&wakers);
                }
                Poll::Pending => {
                    ctx.try_lock()
                        .expect("Concurrent access to the Restate context")
                        .runs
                        .insert(handle, run);
                }
            }
        }

        let mut inner = ctx
            .try_lock()
            .expect("Concurrent access to the Restate context");
        flush(&mut inner)?;
        let mut futures = vec![awaited.clone()];
        futures.extend(inner.runs.keys().copied().map(UnresolvedFuture::Single));
        let unresolved = if futures.len() == 1 {
            futures.pop().unwrap()
        } else {
            UnresolvedFuture::FirstCompleted(futures)
        };
        let progress = inner.vm.do_await(unresolved)?;
        inner.maybe_flip_span_replaying_field();
        match progress {
            AwaitResponse::AnyCompleted => {
                // Registered runs do not have a completion until their proposal
                // is submitted, when they are removed from the execution set.
                return Poll::Ready(Ok(progress));
            }
            AwaitResponse::CancelSignalReceived => {
                inner.runs.clear();
                drop(inner);
                ArcWake::wake_by_ref(&wakers);
                return Poll::Ready(Ok(progress));
            }
            AwaitResponse::ExecuteRun(handle) => {
                let run = inner
                    .runs
                    .get_mut(&handle)
                    .expect("VM requested an unregistered run");
                run.executing = true;
            }
            AwaitResponse::WaitingExternalProgress { waiting_input, .. } => {
                flush(&mut inner)?;
                if !waiting_input {
                    return Poll::Pending;
                }
                match inner.read.poll_recv(&mut shared_cx) {
                    Poll::Ready(Some(Ok(input))) => inner.vm.notify_input(input),
                    Poll::Ready(Some(Err(error))) => inner.vm.notify_error(
                        CoreError::new(500u16, format!("Error when reading the body {error:?}")),
                        None,
                    ),
                    Poll::Ready(None) => inner.vm.notify_input_closed(),
                    Poll::Pending => return Poll::Pending,
                }
                drop(inner);
                ArcWake::wake_by_ref(&wakers);
            }
        }
    }
}
