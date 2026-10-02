use crate::endpoint::ErrorInner;
use crate::endpoint::context::{CONTEXT_LOCK_ERROR, ContextInternalInner};
use futures::future::BoxFuture;
use futures::task::{ArcWake, waker_ref};
use restate_sdk_shared_core::{
    AwaitResponse, Error as CoreError, NotificationHandle, RetryPolicy, RunExitResult,
    UnresolvedFuture, VM,
};
use std::collections::HashMap;
use std::mem;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker, ready};

/// One input stream and all owned run closures share a wakeup source. Input may
/// satisfy a sibling future, including when only completion proposals remain.
/// Wake all subscribers because input and closure readiness do not identify the
/// notification that became ready. Choosing one driver would also require a
/// completion/drop handoff to keep a finished or abandoned waiter from stranding
/// the input stream. The current waiter keeps driving after consuming input, so
/// it does not need a second wakeup for that progress.
#[derive(Default)]
pub(crate) struct ProgressWakers(Mutex<Waiters>);

#[derive(Default)]
struct Waiters {
    next_id: usize,
    wakers: HashMap<usize, Waker>,
}

/// Removes a waiter's wakeup subscription when its future is dropped.
pub(crate) struct ProgressWaiter {
    generation: usize,
    registration: Option<(Arc<ProgressWakers>, usize, Waker)>,
    waiting_progress: Option<(usize, bool)>,
}

impl ProgressWaiter {
    pub(crate) fn new(generation: usize) -> Self {
        Self {
            generation,
            registration: None,
            waiting_progress: None,
        }
    }

    pub(crate) fn generation(&self) -> usize {
        self.generation
    }

    fn register(&mut self, wakers: &Arc<ProgressWakers>, waker: &Waker) {
        if self
            .registration
            .as_ref()
            .is_some_and(|(_, _, registered)| registered.will_wake(waker))
        {
            return;
        }
        let mut waiters = wakers.0.lock().unwrap();
        let id = match &self.registration {
            Some((_, id, _)) => *id,
            None => {
                let id = waiters.next_id;
                waiters.next_id = id
                    .checked_add(1)
                    .expect("Too many waiters in one invocation");
                id
            }
        };
        let entry = waiters.wakers.entry(id).or_insert_with(|| waker.clone());
        if !entry.will_wake(waker) {
            *entry = waker.clone();
        }
        self.registration = Some((Arc::clone(wakers), id, waker.clone()));
    }

    fn wake_siblings(&self, wakers: &ProgressWakers) {
        wakers.wake_waiters(self.registration.as_ref().map(|(_, id, _)| *id));
    }
}

impl Drop for ProgressWaiter {
    fn drop(&mut self) {
        if let Some((wakers, id, _)) = &self.registration {
            wakers.0.lock().unwrap().wakers.remove(id);
        }
    }
}

impl ProgressWakers {
    fn wake_waiters(&self, except: Option<usize>) {
        let waiters = self.0.lock().unwrap();
        if waiters.wakers.len() <= 1 {
            let waiter = waiters
                .wakers
                .iter()
                .find(|(id, _)| Some(**id) != except)
                .map(|(_, waker)| waker.clone());
            drop(waiters);
            if let Some(waiter) = waiter {
                waiter.wake();
            }
            return;
        }
        let wakers: Vec<_> = waiters
            .wakers
            .iter()
            .filter(|(id, _)| Some(**id) != except)
            .map(|(_, waker)| waker.clone())
            .collect();
        drop(waiters);
        for waiter in wakers {
            waiter.wake();
        }
    }
}

impl ArcWake for ProgressWakers {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.wake_waiters(None);
    }
}

pub(crate) struct RegisteredRun {
    pub(crate) future: BoxFuture<'static, Result<RunExitResult, ErrorInner>>,
    pub(crate) retry_policy: RetryPolicy,
}

impl RegisteredRun {
    pub(crate) fn new(
        future: BoxFuture<'static, Result<RunExitResult, ErrorInner>>,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            future,
            retry_policy,
        }
    }
}

pub(crate) fn flush(inner: &mut ContextInternalInner) -> Result<(), ErrorInner> {
    let output = inner.vm.take_output();
    if !output.is_empty() && !inner.write.send(output) {
        return Err(ErrorInner::Suspended);
    }
    Ok(())
}

pub(crate) fn cancel_runs(inner: &mut ContextInternalInner) {
    inner.cancellation_generation += 1;
    inner.runs.clear();
    inner.executing_runs.clear();
}

fn poll_input(inner: &mut ContextInternalInner, cx: &mut Context<'_>) -> Poll<()> {
    match ready!(inner.read.poll_recv(cx)) {
        Some(Ok(input)) => inner.vm.notify_input(input),
        Some(Err(error)) => inner.vm.notify_error(
            CoreError::new(500u16, format!("Error when reading the body {error:?}")),
            None,
        ),
        None => inner.vm.notify_input_closed(),
    }
    inner.progress_version += 1;
    Poll::Ready(())
}

fn includes_cancelled_notification(
    future: &UnresolvedFuture,
    notifications: &HashMap<NotificationHandle, usize>,
    generation: usize,
) -> bool {
    match future {
        UnresolvedFuture::Single(handle) => notifications
            .get(handle)
            .is_some_and(|registered| *registered != generation),
        UnresolvedFuture::Unknown(futures)
        | UnresolvedFuture::FirstCompleted(futures)
        | UnresolvedFuture::AllCompleted(futures)
        | UnresolvedFuture::FirstSucceededOrAllFailed(futures)
        | UnresolvedFuture::AllSucceededOrFirstFailed(futures) => futures
            .iter()
            .any(|future| includes_cancelled_notification(future, notifications, generation)),
        _ => false,
    }
}

/// Drive the VM, owned closures, and input together. User futures are never
/// polled with the VM mutex held.
pub(crate) fn poll_progress(
    ctx: &Arc<Mutex<ContextInternalInner>>,
    cx: &mut Context<'_>,
    waiter: &mut ProgressWaiter,
    awaited: UnresolvedFuture,
) -> Poll<Result<AwaitResponse, ErrorInner>> {
    let mut inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
    let generation = inner.cancellation_generation;
    if waiter.generation != generation
        || includes_cancelled_notification(&awaited, &inner.notifications, generation)
    {
        return Poll::Ready(Ok(AwaitResponse::CancelSignalReceived));
    }
    let wakers = Arc::clone(&inner.progress_wakers);
    waiter.register(&wakers, cx.waker());
    let waker = waker_ref(&wakers);
    let mut shared_cx = Context::from_waker(&waker);

    loop {
        if !inner.executing_runs.is_empty() {
            // Keep the vector's allocation between polls. Run closures may call
            // user code, so move ownership out while the context is unlocked.
            let mut executing = mem::take(&mut inner.executing_runs);
            drop(inner);
            let mut index = 0;
            while index < executing.len() {
                match executing[index].1.future.as_mut().poll(&mut shared_cx) {
                    Poll::Ready(result) => {
                        let (handle, run) = executing.swap_remove(index);
                        let mut inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
                        inner
                            .vm
                            .propose_run_completion(handle, result?, run.retry_policy)?;
                        inner.progress_version += 1;
                        inner.maybe_flip_span_replaying_field();
                        flush(&mut inner)?;
                        drop(inner);
                        waiter.wake_siblings(&wakers);
                    }
                    Poll::Pending => index += 1,
                }
            }
            inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
            if inner.executing_runs.is_empty() {
                inner.executing_runs = executing;
            } else {
                inner.executing_runs.extend(executing);
            }
        }

        flush(&mut inner)?;
        if let Some((version, waiting_input)) = waiter.waiting_progress
            && version == inner.progress_version
            && inner.runs.is_empty()
        {
            // Re-polling the response to drain output is not VM progress. Keep
            // the last input wait parked instead of emitting another Awaiting
            // message. A sibling's input/proposal or a new syscall changes the
            // shared version, so buffered sibling notifications are still driven.
            if !waiting_input {
                return Poll::Pending;
            }
            ready!(poll_input(&mut inner, &mut shared_cx));
            drop(inner);
            waiter.wake_siblings(&wakers);
            inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
        }
        let unresolved = if inner.runs.is_empty() && inner.executing_runs.is_empty() {
            awaited.clone()
        } else {
            let mut futures = Vec::with_capacity(1 + inner.runs.len() + inner.executing_runs.len());
            futures.push(awaited.clone());
            futures.extend(inner.runs.keys().copied().map(UnresolvedFuture::Single));
            futures.extend(
                inner
                    .executing_runs
                    .iter()
                    .map(|(handle, _)| UnresolvedFuture::Single(*handle)),
            );
            UnresolvedFuture::FirstCompleted(futures)
        };
        let progress = inner.vm.do_await(unresolved)?;
        inner.maybe_flip_span_replaying_field();
        match progress {
            AwaitResponse::AnyCompleted => {
                // Only the awaited handles remain eligible for completion after
                // a run has submitted its proposal and left the execution set.
                return Poll::Ready(Ok(progress));
            }
            AwaitResponse::CancelSignalReceived => {
                cancel_runs(&mut inner);
                drop(inner);
                waiter.wake_siblings(&wakers);
                return Poll::Ready(Ok(progress));
            }
            AwaitResponse::ExecuteRun(handle) => {
                waiter.waiting_progress = None;
                let run = inner
                    .runs
                    .remove(&handle)
                    .expect("VM requested an unregistered run");
                inner.executing_runs.push((handle, run));
            }
            AwaitResponse::WaitingExternalProgress { waiting_input, .. } => {
                flush(&mut inner)?;
                waiter.waiting_progress = Some((inner.progress_version, waiting_input));
                if !waiting_input {
                    return Poll::Pending;
                }
                ready!(poll_input(&mut inner, &mut shared_cx));
                drop(inner);
                waiter.wake_siblings(&wakers);
                inner = ctx.try_lock().expect(CONTEXT_LOCK_ERROR);
            }
        }
    }
}
