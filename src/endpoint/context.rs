use crate::context::{Request, RequestTarget, RunClosure, RunRetryPolicy};
use crate::endpoint::futures::async_result_poll::VmAsyncResultPollFuture;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::endpoint::futures::trap::TrapFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{Error, ErrorInner, InputReceiver, OutputSender};
use crate::errors::{HandlerErrorInner, HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use futures::future::Either;
use futures::{FutureExt, TryFutureExt};
use pin_project_lite::pin_project;
use restate_sdk_shared_core::{
    CoreVM, Failure, NonEmptyValue, RetryPolicy, RunEnterResult, RunExitResult, TakeOutputResult,
    Target, Value, VM,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::{ready, Future};
use std::marker::PhantomData;
use std::mem;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant};

pub struct ContextInternalInner {
    pub(crate) vm: CoreVM,
    pub(crate) read: InputReceiver,
    pub(crate) write: OutputSender,
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
            .notify_error(e.0.to_string().into(), format!("{:#}", e.0).into(), None);
        self.handler_state.mark_error(e);
    }
}

/// Internal context interface.
///
/// For the high level interfaces, look at [`crate::context`].
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
                .map(|mut raw_input| {
                    let headers = http::HeaderMap::<String>::try_from(
                        &raw_input
                            .headers
                            .into_iter()
                            .map(|h| (h.key.to_string(), h.value.to_string()))
                            .collect::<HashMap<String, String>>(),
                    )
                    .map_err(|e| {
                        TerminalError::new_with_code(400, format!("Cannot decode headers: {e:?}"))
                    })?;

                    Ok::<_, TerminalError>((
                        T::deserialize(&mut (raw_input.input)).map_err(|e| {
                            TerminalError::new_with_code(
                                400,
                                format!("Cannot decode input payload: {e:?}"),
                            )
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
            Ok(Ok(i)) => {
                drop(inner_lock);
                return Either::Left(ready(i));
            }
            Ok(Err(err)) => {
                let error_inner = ErrorInner::Deserialization {
                    syscall: "input",
                    err: err.0.clone().into(),
                };
                let _ = inner_lock
                    .vm
                    .sys_write_output(NonEmptyValue::Failure(err.into()));
                let _ = inner_lock.vm.sys_end();
                // This causes the trap, plus logs the error
                inner_lock.handler_state.mark_error(error_inner.into());
                drop(inner_lock);
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
                Ok(Value::Void) => Ok(Ok(None)),
                Ok(Value::Success(mut s)) => {
                    let t = T::deserialize(&mut s).map_err(|e| ErrorInner::Deserialization {
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
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
                let _ = inner_lock.vm.sys_state_set(key.to_owned(), b);
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
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

        let maybe_handle = inner_lock.vm.sys_call(request_target.into(), input);
        drop(inner_lock);

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
                Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: "empty",
                    syscall: "call",
                }),
                Ok(Value::Success(mut s)) => {
                    let t = Res::deserialize(&mut s).map_err(|e| ErrorInner::Deserialization {
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
                let _ = inner_lock.vm.sys_send(request_target.into(), t, delay);
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
                Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: "empty",
                    syscall: "awakeable",
                }),
                Ok(Value::Success(mut s)) => {
                    let t = T::deserialize(&mut s).map_err(|e| ErrorInner::Deserialization {
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
                    .sys_complete_awakeable(id.to_owned(), NonEmptyValue::Success(b));
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
                Ok(Value::Void) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: "empty",
                    syscall: "promise",
                }),
                Ok(Value::Success(mut s)) => {
                    let t = T::deserialize(&mut s).map_err(|e| ErrorInner::Deserialization {
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

        let poll_future = VmAsyncResultPollFuture::new(Cow::Borrowed(&self.inner), maybe_handle)
            .map(|res| match res {
                Ok(Value::Void) => Ok(Ok(None)),
                Ok(Value::Success(mut s)) => {
                    let t = T::deserialize(&mut s).map_err(|e| ErrorInner::Deserialization {
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
                    .sys_complete_promise(name.to_owned(), NonEmptyValue::Success(b));
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

    pub fn run<'a, Run, Fut, Out>(
        &'a self,
        run_closure: Run,
    ) -> impl crate::context::RunFuture<Result<Out, TerminalError>> + Send + Sync + 'a
    where
        Run: RunClosure<Fut = Fut, Output = Out> + Send + Sync + 'a,
        Fut: Future<Output = HandlerResult<Out>> + Send + Sync + 'a,
        Out: Serialize + Deserialize + 'static,
    {
        let this = Arc::clone(&self.inner);

        InterceptErrorFuture::new(self.clone(), RunFuture::new(this, run_closure))
    }

    pub fn handle_handler_result<T: Serialize>(&self, res: HandlerResult<T>) {
        let mut inner_lock = must_lock!(self.inner);

        let res_to_write = match res {
            Ok(success) => match T::serialize(&success) {
                Ok(t) => NonEmptyValue::Success(t),
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
            if !inner_lock.write.send(b) {
                // Nothing we can do anymore here
            }
        }
    }

    pub(super) fn fail(&self, e: Error) {
        must_lock!(self.inner).fail(e)
    }
}

pin_project! {
    struct RunFuture<Run, Fut, Ret> {
        name: String,
        retry_policy: RetryPolicy,
        phantom_data: PhantomData<fn() -> Ret>,
        closure: Option<Run>,
        inner_ctx: Option<Arc<Mutex<ContextInternalInner>>>,
        #[pin]
        state: RunState<Fut>,
    }
}

pin_project! {
    #[project = RunStateProj]
    enum RunState<Fut> {
        New,
        ClosureRunning {
            start_time: Instant,
            #[pin]
            fut: Fut,
        },
        PollFutureRunning {
            #[pin]
            fut: VmAsyncResultPollFuture
        }
    }
}

impl<Run, Fut, Ret> RunFuture<Run, Fut, Ret> {
    fn new(inner_ctx: Arc<Mutex<ContextInternalInner>>, closure: Run) -> Self {
        Self {
            name: "".to_string(),
            retry_policy: RetryPolicy::Infinite,
            phantom_data: PhantomData,
            inner_ctx: Some(inner_ctx),
            closure: Some(closure),
            state: RunState::New,
        }
    }
}

impl<Run, Fut, Out> crate::context::RunFuture<Result<Result<Out, TerminalError>, Error>>
    for RunFuture<Run, Fut, Out>
where
    Run: RunClosure<Fut = Fut, Output = Out> + Send + Sync,
    Fut: Future<Output = HandlerResult<Out>> + Send + Sync,
    Out: Serialize + Deserialize,
{
    fn retry_policy(mut self, retry_policy: RunRetryPolicy) -> Self {
        self.retry_policy = RetryPolicy::Exponential {
            initial_interval: retry_policy.initial_delay,
            factor: retry_policy.factor,
            max_interval: retry_policy.max_delay,
            max_attempts: retry_policy.max_attempts,
            max_duration: retry_policy.max_duration,
        };
        self
    }

    fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl<Run, Fut, Out> Future for RunFuture<Run, Fut, Out>
where
    Run: RunClosure<Fut = Fut, Output = Out> + Send + Sync,
    Out: Serialize + Deserialize,
    Fut: Future<Output = HandlerResult<Out>> + Send + Sync,
{
    type Output = Result<Result<Out, TerminalError>, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.state.as_mut().project() {
                RunStateProj::New => {
                    let enter_result = {
                        must_lock!(this
                            .inner_ctx
                            .as_mut()
                            .expect("Future should not be polled after returning Poll::Ready"))
                        .vm
                        .sys_run_enter(this.name.to_owned())
                    };

                    // Enter the side effect
                    match enter_result.map_err(ErrorInner::VM)? {
                        RunEnterResult::Executed(NonEmptyValue::Success(mut v)) => {
                            let t = Out::deserialize(&mut v).map_err(|e| {
                                ErrorInner::Deserialization {
                                    syscall: "run",
                                    err: Box::new(e),
                                }
                            })?;
                            return Poll::Ready(Ok(Ok(t)));
                        }
                        RunEnterResult::Executed(NonEmptyValue::Failure(f)) => {
                            return Poll::Ready(Ok(Err(f.into())))
                        }
                        RunEnterResult::NotExecuted(_) => {}
                    };

                    // We need to run the closure
                    this.state.set(RunState::ClosureRunning {
                        start_time: Instant::now(),
                        fut: this
                            .closure
                            .take()
                            .expect("Future should not be polled after returning Poll::Ready")
                            .run(),
                    });
                }
                RunStateProj::ClosureRunning { start_time, fut } => {
                    let res = match ready!(fut.poll(cx)) {
                        Ok(t) => RunExitResult::Success(Out::serialize(&t).map_err(|e| {
                            ErrorInner::Serialization {
                                syscall: "run",
                                err: Box::new(e),
                            }
                        })?),
                        Err(e) => match e.0 {
                            HandlerErrorInner::Retryable(err) => RunExitResult::RetryableFailure {
                                attempt_duration: start_time.elapsed(),
                                failure: Failure {
                                    code: 500,
                                    message: err.to_string(),
                                },
                            },
                            HandlerErrorInner::Terminal(t) => {
                                RunExitResult::TerminalFailure(TerminalError(t).into())
                            }
                        },
                    };

                    let inner_ctx = this
                        .inner_ctx
                        .take()
                        .expect("Future should not be polled after returning Poll::Ready");

                    let handle = {
                        must_lock!(inner_ctx)
                            .vm
                            .sys_run_exit(res, mem::take(this.retry_policy))
                    };

                    this.state.set(RunState::PollFutureRunning {
                        fut: VmAsyncResultPollFuture::new(Cow::Owned(inner_ctx), handle),
                    });
                }
                RunStateProj::PollFutureRunning { fut } => {
                    let value = ready!(fut.poll(cx))?;

                    return Poll::Ready(match value {
                        Value::Void => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                            variant: "empty",
                            syscall: "run",
                        }
                        .into()),
                        Value::Success(mut s) => {
                            let t = Out::deserialize(&mut s).map_err(|e| {
                                ErrorInner::Deserialization {
                                    syscall: "run",
                                    err: Box::new(e),
                                }
                            })?;
                            Ok(Ok(t))
                        }
                        Value::Failure(f) => Ok(Err(f.into())),
                        Value::StateKeys(_) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                            variant: "state_keys",
                            syscall: "run",
                        }
                        .into()),
                    });
                }
            }
        }
    }
}
