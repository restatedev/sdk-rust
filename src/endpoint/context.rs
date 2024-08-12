use crate::endpoint::futures::{InterceptErrorFuture, TrapFuture};
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{Error, ErrorInner, InputReceiver, OutputSender};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use bytes::Bytes;
use futures::future::Either;
use futures::{FutureExt, TryFutureExt};
use restate_sdk_shared_core::{
    AsyncResultHandle, CoreVM, Header, Input, NonEmptyValue, SuspendedOrVMError, TakeOutputResult,
    VMError, Value, VM,
};
use std::future::{ready, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

pub struct ContextInternalInner {
    vm: CoreVM,
    read: InputReceiver,
    write: OutputSender,
    pub(super) handler_state: HandlerStateNotifier,
}

impl ContextInternalInner {
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
pub struct ContextInternal {
    svc_name: String,
    handler_name: String,
    inner: Arc<Mutex<ContextInternalInner>>,
}

impl ContextInternal {
    pub(super) fn new(
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
            inner: Arc::new(Mutex::new(ContextInternalInner::new(
                vm,
                read,
                write,
                handler_state,
            ))),
        }
    }
}

#[allow(unused)]
const fn is_send_sync<T: Send + Sync>() {}
const _: () = is_send_sync::<ContextInternal>();

macro_rules! must_lock {
    ($mutex:expr) => {
        $mutex.try_lock().expect("You're trying to await two futures at the same time and/or trying to perform some operation on the restate context while awaiting a future. This is not supported!")
    };
}

#[derive(Debug, Eq, PartialEq)]
pub struct InputMetadata {
    pub invocation_id: String,
    pub random_seed: u64,
    pub key: String,
    pub headers: Vec<Header>,
}

impl From<Input> for InputMetadata {
    fn from(value: Input) -> Self {
        Self {
            invocation_id: value.invocation_id,
            random_seed: value.random_seed,
            key: value.key,
            headers: value.headers,
        }
    }
}

impl ContextInternal {
    pub fn service_name(&self) -> &str {
        &self.svc_name
    }

    pub fn handler_name(&self) -> &str {
        &self.handler_name
    }

    pub fn input<T: Deserialize>(&self) -> impl Future<Output = (T, InputMetadata)> {
        let mut inner_lock = must_lock!(self.inner);
        let input_result =
            inner_lock
                .vm
                .sys_input()
                .map_err(ErrorInner::VM)
                .and_then(|raw_input| {
                    Ok((
                        T::deserialize(&mut (raw_input.input.into())).map_err(|e| {
                            ErrorInner::Deserialization {
                                syscall: "input",
                                err: e.into(),
                            }
                        })?,
                        InputMetadata {
                            invocation_id: raw_input.invocation_id,
                            random_seed: raw_input.random_seed,
                            key: raw_input.key,
                            headers: raw_input.headers,
                        },
                    ))
                });

        match input_result {
            Ok(i) => {
                drop(inner_lock);
                return Either::Left(ready(i));
            }
            Err(e) => {
                inner_lock.handler_state.mark_error_inner(e);
                drop(inner_lock);
            }
        }
        Either::Right(TrapFuture::default())
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

        InterceptErrorFuture::new(self.clone(), poll_future.map_err(Error))
    }

    pub fn get<T: Deserialize>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_state_get(key.to_owned()) };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(None)),
            Ok(Value::Success(s)) => {
                let mut b = Bytes::from(s);
                let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                    syscall: "get_state",
                    err: Box::new(e),
                })?;
                Ok(Ok(Some(t)))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "get_state",
            }),
            Err(e) => Err(e),
        });

        InterceptErrorFuture::new(self.clone(), poll_future.map_err(Error))
    }

    pub fn set<T: Serialize>(&self, key: &str, t: T) {
        let mut inner_lock = must_lock!(self.inner);
        match t.serialize() {
            Ok(b) => {
                let _ = inner_lock.vm.sys_state_set(key.to_owned(), b.to_vec());
            }
            Err(e) => {
                inner_lock
                    .handler_state
                    .mark_error_inner(ErrorInner::Serialization {
                        syscall: "set_state",
                        err: Box::new(e),
                    });
            }
        }
    }

    pub fn clear(&self, key: &str) {
        let _ = must_lock!(self.inner).vm.sys_state_clear(key.to_string());
    }

    fn create_poll_future(
        &self,
        handle: Result<AsyncResultHandle, VMError>,
    ) -> impl Future<Output = Result<Value, ErrorInner>> + Send + Sync {
        VmPollFuture {
            state: Some(match handle {
                Ok(handle) => PollState::Init {
                    ctx: Arc::clone(&self.inner),
                    handle,
                },
                Err(err) => PollState::Failed(ErrorInner::VM(err)),
            }),
        }
    }

    pub fn write_output<T: Serialize>(&self, res: HandlerResult<T>) {
        let mut inner_lock = must_lock!(self.inner);

        let res_to_write = match res {
            Ok(success) => match T::serialize(&success) {
                Ok(t) => NonEmptyValue::Success(t.to_vec()),
                Err(e) => {
                    inner_lock
                        .handler_state
                        .mark_error_inner(ErrorInner::Serialization {
                            syscall: "output",
                            err: Box::new(e),
                        });
                    return;
                }
            },
            Err(f) => NonEmptyValue::Failure(f.into()),
        };

        let _ = inner_lock.vm.sys_write_output(res_to_write);
    }

    pub fn end(&self) {
        let _ = must_lock!(self.inner).vm.sys_end();
    }

    pub(crate) fn consume_to_end(&self) {
        let mut inner_lock = must_lock!(self.inner);

        let out = inner_lock.vm.take_output();
        if let TakeOutputResult::Buffer(b) = out {
            if !inner_lock.write.send(b.into()) {
                // Nothing we can do anymore here
            }
        }
    }

    pub(super) fn fail(&self, e: Error) {
        must_lock!(self.inner).handler_state.mark_error(e);
    }
}

struct VmPollFuture {
    state: Option<PollState>,
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

impl Future for VmPollFuture {
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
                        Some(Ok(b)) => inner_lock.vm.notify_input(b.to_vec()),
                        Some(Err(e)) => inner_lock.vm.notify_error(
                            "Error when reading the body".into(),
                            e.to_string().into(),
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
