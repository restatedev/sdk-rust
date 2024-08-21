use crate::context::{Request, RequestTarget, RunClosure};
use crate::endpoint::futures::{InterceptErrorFuture, TrapFuture};
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{Error, ErrorInner, InputReceiver, OutputSender};
use crate::errors::{HandlerErrorInner, HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use bytes::Bytes;
use futures::future::Either;
use futures::{FutureExt, TryFutureExt};
use restate_sdk_shared_core::{
    AsyncResultHandle, CoreVM, NonEmptyValue, RunEnterResult, SuspendedOrVMError, TakeOutputResult,
    Target, VMError, Value, VM,
};
use std::collections::HashMap;
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

    pub(super) fn fail(&mut self, e: Error) {
        self.vm
            .notify_error(e.0.to_string().into(), format!("{:#}", e.0).into());
        self.handler_state.mark_error(e);
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
    pub headers: http::HeaderMap<String>,
}

impl From<RequestTarget> for Target {
    fn from(value: RequestTarget) -> Self {
        match value {
            RequestTarget::Service { name, handler } => Target {
                service: name,
                handler,
                key: None,
            },
            RequestTarget::Object { name, key, handler } => Target {
                service: name,
                handler,
                key: Some(key),
            },
            RequestTarget::Workflow { name, key, handler } => Target {
                service: name,
                handler,
                key: Some(key),
            },
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
                    let headers = http::HeaderMap::<String>::try_from(
                        &raw_input
                            .headers
                            .into_iter()
                            .map(|h| (h.key.to_string(), h.value.to_string()))
                            .collect::<HashMap<String, String>>(),
                    )
                    .map_err(|e| ErrorInner::Deserialization {
                        syscall: "input_headers",
                        err: e.into(),
                    })?;

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
                            headers,
                        },
                    ))
                });

        match input_result {
            Ok(i) => {
                drop(inner_lock);
                return Either::Left(ready(i));
            }
            Err(e) => {
                inner_lock.fail(e.into());
                drop(inner_lock);
            }
        }
        Either::Right(TrapFuture::default())
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

    pub fn get_keys(
        &self,
    ) -> impl Future<Output = Result<Vec<String>, TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_state_get_keys() };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "empty",
                syscall: "get_state",
            }),
            Ok(Value::Success(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "success",
                syscall: "get_state",
            }),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(s)) => Ok(Ok(s)),
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
                inner_lock.fail(
                    ErrorInner::Serialization {
                        syscall: "set_state",
                        err: Box::new(e),
                    }
                    .into(),
                );
            }
        }
    }

    pub fn clear(&self, key: &str) {
        let _ = must_lock!(self.inner).vm.sys_state_clear(key.to_string());
    }

    pub fn clear_all(&self) {
        let _ = must_lock!(self.inner).vm.sys_state_clear_all();
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

    pub fn request<Req, Res>(&self, request_target: RequestTarget, req: Req) -> Request<Req, Res> {
        Request::new(self, request_target, req)
    }

    pub fn call<Req: Serialize, Res: Deserialize>(
        &self,
        request_target: RequestTarget,
        req: Req,
    ) -> impl Future<Output = Result<Res, TerminalError>> + Send + Sync {
        let mut inner_lock = must_lock!(self.inner);

        let input = match Req::serialize(&req) {
            Ok(t) => t,
            Err(e) => {
                inner_lock.fail(
                    ErrorInner::Serialization {
                        syscall: "call",
                        err: Box::new(e),
                    }
                    .into(),
                );
                return Either::Right(TrapFuture::default());
            }
        };

        let maybe_handle = inner_lock
            .vm
            .sys_call(request_target.into(), input.to_vec());
        drop(inner_lock);

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "empty",
                syscall: "call",
            }),
            Ok(Value::Success(s)) => {
                let mut b = Bytes::from(s);
                let t = Res::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                    syscall: "call",
                    err: Box::new(e),
                })?;
                Ok(Ok(t))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "call",
            }),
            Err(e) => Err(e),
        });

        Either::Left(InterceptErrorFuture::new(
            self.clone(),
            poll_future.map_err(Error),
        ))
    }

    pub fn send<Req: Serialize>(
        &self,
        request_target: RequestTarget,
        req: Req,
        delay: Option<Duration>,
    ) {
        let mut inner_lock = must_lock!(self.inner);

        match Req::serialize(&req) {
            Ok(t) => {
                let _ = inner_lock
                    .vm
                    .sys_send(request_target.into(), t.to_vec(), delay);
            }
            Err(e) => {
                inner_lock.fail(
                    ErrorInner::Serialization {
                        syscall: "call",
                        err: Box::new(e),
                    }
                    .into(),
                );
            }
        };
    }

    pub fn awakeable<T: Deserialize>(
        &self,
    ) -> (
        String,
        impl Future<Output = Result<T, TerminalError>> + Send + Sync,
    ) {
        let maybe_awakeable_id_and_handle = { must_lock!(self.inner).vm.sys_awakeable() };

        let (awakeable_id, maybe_handle) = match maybe_awakeable_id_and_handle {
            Ok((s, handle)) => (s, Ok(handle)),
            Err(e) => (
                // TODO NOW this is REALLY BAD. The reason for this is that we would need to return a future of a future instead, which is not nice.
                //  we assume for the time being this works because no user should use the awakeable without doing any other syscall first, which will prevent this invalid awakeable id to work in the first place.
                "invalid".to_owned(),
                Err(e),
            ),
        };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "empty",
                syscall: "awakeable",
            }),
            Ok(Value::Success(s)) => {
                let mut b = Bytes::from(s);
                let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                    syscall: "awakeable",
                    err: Box::new(e),
                })?;
                Ok(Ok(t))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "awakeable",
            }),
            Err(e) => Err(e),
        });

        (
            awakeable_id,
            InterceptErrorFuture::new(self.clone(), poll_future.map_err(Error)),
        )
    }

    pub fn resolve_awakeable<T: Serialize>(&self, id: &str, t: T) {
        let mut inner_lock = must_lock!(self.inner);
        match t.serialize() {
            Ok(b) => {
                let _ = inner_lock
                    .vm
                    .sys_complete_awakeable(id.to_owned(), NonEmptyValue::Success(b.to_vec()));
            }
            Err(e) => {
                inner_lock.fail(
                    ErrorInner::Serialization {
                        syscall: "resolve_awakeable",
                        err: Box::new(e),
                    }
                    .into(),
                );
            }
        }
    }

    pub fn reject_awakeable(&self, id: &str, failure: TerminalError) {
        let _ = must_lock!(self.inner)
            .vm
            .sys_complete_awakeable(id.to_owned(), NonEmptyValue::Failure(failure.into()));
    }

    pub fn promise<T: Deserialize>(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<T, TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_get_promise(name.to_owned()) };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "empty",
                syscall: "promise",
            }),
            Ok(Value::Success(s)) => {
                let mut b = Bytes::from(s);
                let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                    syscall: "promise",
                    err: Box::new(e),
                })?;
                Ok(Ok(t))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "promise",
            }),
            Err(e) => Err(e),
        });

        InterceptErrorFuture::new(self.clone(), poll_future.map_err(Error))
    }

    pub fn peek_promise<T: Deserialize>(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + Send + Sync {
        let maybe_handle = { must_lock!(self.inner).vm.sys_peek_promise(name.to_owned()) };

        let poll_future = self.create_poll_future(maybe_handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(None)),
            Ok(Value::Success(s)) => {
                let mut b = Bytes::from(s);
                let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                    syscall: "peek_promise",
                    err: Box::new(e),
                })?;
                Ok(Ok(Some(t)))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(_)) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: "state_keys",
                syscall: "peek_promise",
            }),
            Err(e) => Err(e),
        });

        InterceptErrorFuture::new(self.clone(), poll_future.map_err(Error))
    }

    pub fn resolve_promise<T: Serialize>(&self, name: &str, t: T) {
        let mut inner_lock = must_lock!(self.inner);
        match t.serialize() {
            Ok(b) => {
                let _ = inner_lock
                    .vm
                    .sys_complete_promise(name.to_owned(), NonEmptyValue::Success(b.to_vec()));
            }
            Err(e) => {
                inner_lock.fail(
                    ErrorInner::Serialization {
                        syscall: "resolve_promise",
                        err: Box::new(e),
                    }
                    .into(),
                );
            }
        }
    }

    pub fn reject_promise(&self, id: &str, failure: TerminalError) {
        let _ = must_lock!(self.inner)
            .vm
            .sys_complete_promise(id.to_owned(), NonEmptyValue::Failure(failure.into()));
    }

    pub fn run<'a, R, F, T>(
        &'a self,
        name: &'a str,
        run_closure: R,
    ) -> impl Future<Output = Result<T, TerminalError>> + Send + Sync + 'a
    where
        R: RunClosure<Fut = F, Output = T> + Send + Sync + 'a,
        T: Serialize + Deserialize,
        F: Future<Output = HandlerResult<T>> + Send + Sync + 'a,
    {
        let this = Arc::clone(&self.inner);

        InterceptErrorFuture::new(self.clone(), async move {
            let enter_result = { must_lock!(this).vm.sys_run_enter(name.to_owned()) };

            // Enter the side effect
            match enter_result.map_err(ErrorInner::VM)? {
                RunEnterResult::Executed(NonEmptyValue::Success(v)) => {
                    let mut b = Bytes::from(v);
                    let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                        syscall: "run",
                        err: Box::new(e),
                    })?;
                    return Ok(Ok(t));
                }
                RunEnterResult::Executed(NonEmptyValue::Failure(f)) => return Ok(Err(f.into())),
                RunEnterResult::NotExecuted => {}
            };

            // We need to run the closure
            let res = match run_closure.run().await {
                Ok(t) => NonEmptyValue::Success(
                    T::serialize(&t)
                        .map_err(|e| ErrorInner::Serialization {
                            syscall: "run",
                            err: Box::new(e),
                        })?
                        .to_vec(),
                ),
                Err(e) => match e.0 {
                    HandlerErrorInner::Retryable(err) => {
                        return Err(ErrorInner::RunResult {
                            name: name.to_owned(),
                            err,
                        }
                        .into())
                    }
                    HandlerErrorInner::Terminal(t) => {
                        NonEmptyValue::Failure(TerminalError(t).into())
                    }
                },
            };

            let handle = {
                must_lock!(this)
                    .vm
                    .sys_run_exit(res)
                    .map_err(ErrorInner::VM)?
            };

            let value = self.create_poll_future(Ok(handle)).await?;

            match value {
                Value::Void => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: "empty",
                    syscall: "run",
                }
                .into()),
                Value::Success(s) => {
                    let mut b = Bytes::from(s);
                    let t = T::deserialize(&mut b).map_err(|e| ErrorInner::Deserialization {
                        syscall: "run",
                        err: Box::new(e),
                    })?;
                    Ok(Ok(t))
                }
                Value::Failure(f) => Ok(Err(f.into())),
                Value::StateKeys(_) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: "state_keys",
                    syscall: "run",
                }
                .into()),
            }
        })
    }

    pub fn handle_handler_result<T: Serialize>(&self, res: HandlerResult<T>) {
        let mut inner_lock = must_lock!(self.inner);

        let res_to_write = match res {
            Ok(success) => match T::serialize(&success) {
                Ok(t) => NonEmptyValue::Success(t.to_vec()),
                Err(e) => {
                    inner_lock.fail(
                        ErrorInner::Serialization {
                            syscall: "output",
                            err: Box::new(e),
                        }
                        .into(),
                    );
                    return;
                }
            },
            Err(e) => match e.0 {
                HandlerErrorInner::Retryable(err) => {
                    inner_lock.fail(ErrorInner::HandlerResult { err }.into());
                    return;
                }
                HandlerErrorInner::Terminal(t) => NonEmptyValue::Failure(TerminalError(t).into()),
            },
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
        must_lock!(self.inner).fail(e)
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
