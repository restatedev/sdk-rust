use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{ErrorInner, InputReceiver, OutputSender};
use crate::errors::TerminalError;
use futures::future::Either;
use futures::FutureExt;
use pin_project_lite::pin_project;
use restate_sdk_shared_core::{
    AsyncResultHandle, CoreVM, Input, NonEmptyValue, SuspendedOrVMError, TakeOutputResult, VMError,
    Value, VM,
};
use std::future::{ready, Future};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};
use std::time::Duration;

pub struct HandlerContextInner {
    vm: CoreVM,
    read: InputReceiver,
    write: OutputSender,
    handler_state: HandlerStateNotifier,
}

impl HandlerContextInner {
    fn new(
        vm: CoreVM,
        read: InputReceiver,
        write: OutputSender,
        handler_state: HandlerStateNotifier,
    ) -> Self {
        Self {
            vm,
            read,
            write,
            handler_state,
        }
    }
}

#[derive(Clone)]
pub struct HandlerContext {
    svc_name: String,
    handler_name: String,
    inner: Arc<Mutex<HandlerContextInner>>,
}

impl HandlerContext {
    pub(crate) fn new(
        vm: CoreVM,
        svc_name: String,
        handler_name: String,
        read: InputReceiver,
        write: OutputSender,
        handler_state: HandlerStateNotifier,
    ) -> Self {
        Self {
            svc_name,
            handler_name,
            inner: Arc::new(Mutex::new(HandlerContextInner::new(
                vm,
                read,
                write,
                handler_state,
            )))
        }
    }
}

#[allow(unused)]
const fn is_send_sync<T: Send + Sync>() {}
const _: () = is_send_sync::<HandlerContext>();

macro_rules! must_lock {
    ($mutex:expr) => {
        $mutex.try_lock().expect("You're trying to await two futures at the same time and/or trying to perform some operation on the restate context while awaiting a future. This is not supported!")
    };
}

impl HandlerContext {
    pub fn handler_name(&self) -> &str {
        &self.handler_name
    }

    pub fn input(&self) -> impl Future<Output = Input> {
        let mut inner_lock = must_lock!(self.inner);
        match inner_lock.vm.sys_input() {
            Ok(i) => {
                drop(inner_lock);
                return Either::Left(ready(i));
            }
            Err(e) => {
                inner_lock.handler_state.mark_error_inner(ErrorInner::VM(e));
                drop(inner_lock);
            }
        }
        Either::Right(PendingAwakeNow(Default::default()))
    }

    pub fn sleep(
        &self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_sleep(duration) };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(())),
            Ok(Value::Success(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "success",
                syscall: "sleep",
            }),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Err(e) => Err(e),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "sleep",
            }),
        });

        InterceptErrorFuture {
            fut: poll_future,
            ctx: Arc::clone(&self.inner),
        }
    }

    pub fn get_state(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_state_get(key.to_owned()) };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(None)),
            Ok(Value::Success(s)) => Ok(Ok(Some(s))),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "get_state",
            }),
            Err(e) => Err(e),
        });

        InterceptErrorFuture {
            fut: poll_future,
            ctx: Arc::clone(&self.inner),
        }
    }

    pub fn set_state(&self, key: String, value: Vec<u8>) {
        let _ = must_lock!(self.inner).vm.sys_state_set(key, value);
    }

    fn create_poll_future(
        &self,
        handle: Result<AsyncResultHandle, VMError>,
    ) -> impl Future<Output = Result<Value, ErrorInner>> + Send + Sync {
        return VmPollFuture {
            state: Some(match handle {
                Ok(handle) => PollState::Init {
                    ctx: Arc::clone(&self.inner),
                    handle,
                },
                Err(err) => PollState::Failed(ErrorInner::VM(err)),
            }),
        };
    }

    pub fn write_output(&self, res: Result<Vec<u8>, TerminalError>) {
        let _ = must_lock!(self.inner).vm.sys_write_output(match res {
            Ok(success) => NonEmptyValue::Success(success),
            Err(f) => NonEmptyValue::Failure(f.into()),
        });
    }

    pub fn consume_to_end(self) {
        let mut inner_lock = must_lock!(self.inner);

        let out = inner_lock.vm.take_output();
        match out {
            TakeOutputResult::Buffer(b) => {
                if !inner_lock.write.send(b.into()) {
                    // Nothing we can do anymore here
                }
            }
            _ => {}
        }
    }

    // pub fn as_context(self) {
    //
    // }
    //
    // pub fn as_object_context(self) {
    //
    // }
    //
    // pub fn as_shared_object_context(self) {
    //
    // }
}

struct VmPollFuture {
    state: Option<PollState>,
}

enum PollState {
    Init {
        ctx: Arc<Mutex<HandlerContextInner>>,
        handle: AsyncResultHandle,
    },
    WaitingInput {
        ctx: Arc<Mutex<HandlerContextInner>>,
        handle: AsyncResultHandle,
    },
    Failed(ErrorInner),
}

impl Future for VmPollFuture {
    type Output = Result<Value, ErrorInner>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
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
                            if !inner_lock.write.send(b.into()) {
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

                    let read_result = ready!(inner_lock.read.poll_recv(cx));

                    // Pass read result to VM
                    if let Some(b) = read_result {
                        inner_lock.vm.notify_input(b.to_vec());
                    } else {
                        inner_lock.vm.notify_input_closed();
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

struct PendingAwakeNow<T>(PhantomData<T>);

impl<T> Future for PendingAwakeNow<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<T> {
        ctx.waker().wake_by_ref();
        Poll::Pending
    }
}

pin_project! {
    struct InterceptErrorFuture<F>{
        #[pin]
        fut: F,
        ctx: Arc<Mutex<HandlerContextInner>>
    }
}

impl<F, R> Future for InterceptErrorFuture<F>
where
    F: Future<Output = Result<R, ErrorInner>>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        let result = ready!(this.fut.poll(cx));

        match result {
            Ok(r) => Poll::Ready(r),
            Err(e) => {
                {
                    let mut inner_lock = must_lock!(&mut this.ctx);
                    inner_lock.handler_state.mark_error_inner(e);
                }

                // Here is the secret sauce. This will immediately cause the whole future chain to be polled,
                //  but the poll here will be intercepted by HandlerStateAwareFuture
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}
