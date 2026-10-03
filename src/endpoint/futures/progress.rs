use crate::endpoint::ErrorInner;
use crate::endpoint::context::{CONTEXT_LOCK_ERROR, ContextInternalInner, ContextShared};
use futures::future::BoxFuture;
use futures::task::{ArcWake, waker_ref};
use restate_sdk_shared_core::{
    AwaitResponse, Error as CoreError, NotificationHandle, RetryPolicy, RunExitResult,
    UnresolvedFuture, VM,
};
use std::collections::HashMap;
use std::mem;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWakerVTable, Waker, ready};

/// One input stream and all owned run closures share a wakeup source. Input may
/// satisfy a sibling future, including when only completion proposals remain.
/// Wake all subscribers because input and closure readiness do not identify the
/// notification that became ready. Choosing one driver would also require a
/// completion/drop handoff to keep a finished or abandoned waiter from stranding
/// the input stream. The current waiter keeps driving after consuming input, so
/// it does not need a second wakeup for that progress.
/// `tests/started_run_benchmark.rs` measures this strategy for N = 1, 10, 100,
/// and 1000; `tests/sequential_context_benchmark.rs` checks ordinary awaits.
#[derive(Default)]
pub(crate) struct ProgressWakers {
    waiters: Mutex<Waiters>,
    subscriber_count: AtomicUsize,
}

#[derive(Default)]
struct Waiters {
    next_id: usize,
    first: Option<(usize, Waker)>,
    wakers: HashMap<usize, Waker>,
}

impl Waiters {
    fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.wakers.len()
    }
}

/// Keeps the context alive until its wakeup subscription has been removed.
pub(crate) struct ProgressGuard {
    ctx: Option<Arc<ContextShared>>,
    waiter: ProgressWaiter,
}

impl ProgressGuard {
    pub(crate) fn new(ctx: Arc<ContextShared>, generation: usize) -> Self {
        Self {
            ctx: Some(ctx),
            waiter: ProgressWaiter::new(generation),
        }
    }

    pub(crate) fn context(&self) -> &Arc<ContextShared> {
        self.ctx.as_ref().expect("Progress guard has a context")
    }

    pub(crate) fn generation(&self) -> usize {
        self.waiter.generation
    }

    pub(crate) fn take_context(&mut self) -> Arc<ContextShared> {
        // A borrowed closure hands its owner to the result future only after
        // leaving the wake registry.
        let ctx = self.ctx.as_ref().expect("Progress guard has a context");
        self.waiter.unregister(&ctx.progress_wakers);
        self.ctx.take().expect("Progress guard has a context")
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if let Some(ctx) = &self.ctx {
            // Release the registry lock before dropping the context owner:
            // context teardown can itself drop other progress guards.
            self.waiter.unregister(&ctx.progress_wakers);
        }
    }
}

struct ProgressWaiter {
    generation: usize,
    registration: Option<(usize, WakerIdentity)>,
    waiting_progress: Option<(usize, bool)>,
}

struct WakerIdentity {
    data: usize,
    vtable: &'static RawWakerVTable,
}

impl WakerIdentity {
    fn new(waker: &Waker) -> Self {
        Self {
            data: waker.data().addr(),
            vtable: waker.vtable(),
        }
    }

    fn will_wake(&self, waker: &Waker) -> bool {
        self.data == waker.data().addr() && std::ptr::eq(self.vtable, waker.vtable())
    }
}

impl ProgressWaiter {
    fn new(generation: usize) -> Self {
        Self {
            generation,
            registration: None,
            waiting_progress: None,
        }
    }

    fn register(&mut self, wakers: &ProgressWakers, waker: &Waker) {
        if self
            .registration
            .as_ref()
            .is_some_and(|(_, registered)| registered.will_wake(waker))
        {
            return;
        }
        let mut waiters = wakers.waiters.lock().unwrap();
        let id = match &self.registration {
            Some((id, _)) => *id,
            None => {
                let id = waiters.next_id;
                waiters.next_id = id
                    .checked_add(1)
                    .expect("Too many waiters in one invocation");
                id
            }
        };
        // Sample the owned clone: a custom waker's clone may use a different
        // allocation. The registry retains that allocation for the subscription,
        // so the cached identity matches std::Waker::will_wake without a clone.
        let registered_waker = waker.clone();
        let identity = WakerIdentity::new(&registered_waker);
        match &mut waiters.first {
            Some((first_id, registered)) if *first_id == id => {
                *registered = registered_waker;
            }
            None => waiters.first = Some((id, registered_waker)),
            Some(_) => {
                waiters.wakers.insert(id, registered_waker);
            }
        }
        if self.registration.is_none() {
            wakers
                .subscriber_count
                .store(waiters.len(), Ordering::Release);
        }
        self.registration = Some((id, identity));
    }

    fn wake_siblings(&self, wakers: &ProgressWakers) {
        if wakers.subscriber_count.load(Ordering::Acquire) <= 1 {
            return;
        }
        wakers.wake_waiters(self.registration.as_ref().map(|(id, _)| *id));
    }

    fn unregister(&mut self, wakers: &ProgressWakers) {
        if let Some((id, _)) = self.registration.take() {
            let mut waiters = wakers.waiters.lock().unwrap();
            if waiters
                .first
                .as_ref()
                .is_some_and(|(first, _)| *first == id)
            {
                let next = waiters.wakers.keys().next().copied();
                waiters.first = next.map(|next| (next, waiters.wakers.remove(&next).unwrap()));
            } else {
                waiters.wakers.remove(&id);
            }
            wakers
                .subscriber_count
                .store(waiters.len(), Ordering::Release);
        }
    }
}

impl ProgressWakers {
    fn wake_waiters(&self, except: Option<usize>) {
        let waiters = self.waiters.lock().unwrap();
        if waiters.wakers.is_empty() {
            let waiter = waiters
                .first
                .as_ref()
                .filter(|(id, _)| Some(*id) != except)
                .map(|(_, waker)| waker.clone());
            drop(waiters);
            if let Some(waiter) = waiter {
                waiter.wake();
            }
            return;
        }
        let wakers: Vec<_> = waiters
            .first
            .iter()
            .map(|(id, waker)| (id, waker))
            .chain(waiters.wakers.iter())
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
    inner.progress_version += 1;
    inner.runs.clear();
    inner.executing_runs.clear();
}

fn poll_input(
    inner: &mut ContextInternalInner,
    cx: &mut Context<'_>,
    shared_cx: &mut Context<'_>,
    wakers: &ProgressWakers,
) -> Poll<()> {
    // A sole waiter can register its executor waker directly. A new pending
    // sibling registers before polling input and installs the shared waker,
    // so dropping either waiter cannot leave the other without an input wakeup.
    let input = if inner.runs.is_empty()
        && inner.executing_runs.is_empty()
        && wakers.subscriber_count.load(Ordering::Acquire) == 1
    {
        inner.read.poll_recv(cx)
    } else {
        inner.read.poll_recv(shared_cx)
    };
    match ready!(input) {
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

#[derive(Clone, Copy)]
pub(crate) enum Awaited<'a> {
    Single(NotificationHandle),
    FirstCompleted(&'a [NotificationHandle]),
}

impl Awaited<'_> {
    fn handles(&self) -> &[NotificationHandle] {
        match self {
            Self::Single(handle) => std::slice::from_ref(handle),
            Self::FirstCompleted(handles) => handles,
        }
    }

    fn into_unresolved(self) -> UnresolvedFuture {
        match self {
            Self::Single(handle) => UnresolvedFuture::Single(handle),
            Self::FirstCompleted(handles) => UnresolvedFuture::FirstCompleted(
                handles
                    .iter()
                    .copied()
                    .map(UnresolvedFuture::Single)
                    .collect(),
            ),
        }
    }
}

fn includes_cancelled_notification(
    future: Awaited<'_>,
    notifications: &HashMap<NotificationHandle, usize>,
    generation: usize,
) -> bool {
    generation != 0
        && future
            .handles()
            .iter()
            .any(|handle| notifications.get(handle).copied().unwrap_or(0) != generation)
}

fn is_completed(future: Awaited<'_>, vm: &impl VM) -> bool {
    future
        .handles()
        .iter()
        .any(|handle| vm.is_completed(*handle))
}

pub(crate) enum ProgressResult<T> {
    Completed(T),
    Cancelled,
}

/// Drive the VM, owned closures, and input together. User futures are never
/// polled with the VM mutex held.
pub(crate) fn poll_progress<T>(
    guard: &mut ProgressGuard,
    cx: &mut Context<'_>,
    awaited: Awaited<'_>,
    completed: impl FnOnce(&mut ContextInternalInner) -> Result<T, ErrorInner>,
) -> Poll<Result<ProgressResult<T>, ErrorInner>> {
    let ProgressGuard { ctx, waiter } = guard;
    let ctx = ctx.as_ref().expect("Progress guard has a context");
    let mut inner = ctx.inner.try_lock().expect(CONTEXT_LOCK_ERROR);
    let generation = inner.cancellation_generation;
    let cancelled = waiter.generation != generation
        || includes_cancelled_notification(awaited, &inner.notifications, generation);
    let unchanged_wait = waiter
        .waiting_progress
        .is_some_and(|(version, _)| version == inner.progress_version);
    // A cached wait drained the notification queue and checked readiness. With
    // no new progress its target stays unready; stale cancellation epochs still
    // check known results before settling cancellation.
    if (cancelled || inner.may_have_pending_invocation_ids && !unchanged_wait)
        && is_completed(awaited, &inner.vm)
    {
        flush(&mut inner)?;
        return Poll::Ready(completed(&mut inner).map(ProgressResult::Completed));
    }
    if cancelled {
        return Poll::Ready(Ok(ProgressResult::Cancelled));
    }
    let wakers = &ctx.progress_wakers;
    waiter.register(wakers, cx.waker());
    let waker = waker_ref(wakers);
    let mut shared_cx = Context::from_waker(&waker);

    // The last WaitingExternalProgress drained output before caching this
    // version. Every input, proposal, and context command changes the version,
    // so an unchanged parked waiter has no new output to drain on entry.
    if !unchanged_wait {
        flush(&mut inner)?;
    }
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
                        let mut inner = ctx.inner.try_lock().expect(CONTEXT_LOCK_ERROR);
                        inner
                            .vm
                            .propose_run_completion(handle, result?, run.retry_policy)?;
                        inner.progress_version += 1;
                        inner.maybe_flip_span_replaying_field();
                        flush(&mut inner)?;
                        drop(inner);
                        waiter.wake_siblings(wakers);
                    }
                    Poll::Pending => index += 1,
                }
            }
            inner = ctx.inner.try_lock().expect(CONTEXT_LOCK_ERROR);
            if inner.executing_runs.is_empty() {
                inner.executing_runs = executing;
            } else {
                inner.executing_runs.extend(executing);
            }
            // Pending user futures can also buffer synchronous commands while
            // the context is unlocked, even without submitting a proposal.
            flush(&mut inner)?;
        }

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
            ready!(poll_input(&mut inner, cx, &mut shared_cx, wakers));
            if wakers.subscriber_count.load(Ordering::Acquire) > 1 {
                drop(inner);
                waiter.wake_siblings(wakers);
                inner = ctx.inner.try_lock().expect(CONTEXT_LOCK_ERROR);
                // A sibling's wake callback can buffer synchronous commands
                // while the context is unlocked; drain them on reacquisition.
                flush(&mut inner)?;
            }
        }
        let unresolved = if inner.runs.is_empty() && inner.executing_runs.is_empty() {
            awaited.into_unresolved()
        } else {
            let mut futures = Vec::with_capacity(1 + inner.runs.len() + inner.executing_runs.len());
            futures.push(awaited.into_unresolved());
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
                // Shared-core's ready transition emits no output. Commands are
                // drained on entry and after every unlocked user-code/wake gap.
                return Poll::Ready(completed(&mut inner).map(ProgressResult::Completed));
            }
            AwaitResponse::CancelSignalReceived => {
                cancel_runs(&mut inner);
                drop(inner);
                waiter.wake_siblings(wakers);
                return Poll::Ready(Ok(ProgressResult::Cancelled));
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
                // Cancellation can wait for outstanding call invocation IDs.
                // Resolving those IDs can consume a sibling notification and
                // make this result ready without settling cancellation yet.
                if inner.may_have_pending_invocation_ids && is_completed(awaited, &inner.vm) {
                    return Poll::Ready(completed(&mut inner).map(ProgressResult::Completed));
                }
                waiter.waiting_progress = Some((inner.progress_version, waiting_input));
                if !waiting_input {
                    return Poll::Pending;
                }
                ready!(poll_input(&mut inner, cx, &mut shared_cx, wakers));
                if wakers.subscriber_count.load(Ordering::Acquire) > 1 {
                    drop(inner);
                    waiter.wake_siblings(wakers);
                    inner = ctx.inner.try_lock().expect(CONTEXT_LOCK_ERROR);
                    flush(&mut inner)?;
                }
            }
        }
    }
}
