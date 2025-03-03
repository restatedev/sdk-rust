use crate::context::{
    CallFuture, InvocationHandle, Request, RequestTarget, RunClosure, RunRetryPolicy,
};
use crate::endpoint::futures::async_result_poll::VmAsyncResultPollFuture;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::endpoint::futures::trap::TrapFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{Error, ErrorInner, InputReceiver, OutputSender};
use crate::errors::{HandlerErrorInner, HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use futures::future::{Either, Shared};
use futures::{FutureExt, TryFutureExt};
use pin_project_lite::pin_project;
use restate_sdk_shared_core::{
    CallHandle as CoreCallHandle, CoreVM, DoProgressResponse, Error as CoreError, NonEmptyValue,
    NotificationHandle, RetryPolicy, RunExitResult, SendHandle as CoreSendHandle, TakeOutputResult,
    Target, TerminalFailure, Value, VM,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::{ready, Future};
use std::marker::PhantomData;
use std::mem;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant, SystemTime};

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
        self.vm.notify_error(
            CoreError::new(500u16, e.0.to_string()).with_stacktrace(Cow::Owned(format!("{:#}", e.0))),
            None,
        );
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

macro_rules! unwrap_or_trap {
    ($inner_lock:expr, $res:expr) => {
        match $res {
            Ok(t) => t,
            Err(e) => {
                $inner_lock.fail(e.into());
                return Either::Right(TrapFuture::default());
            }
        }
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
                idempotency_key: None,
                headers: vec![],
            },
            RequestTarget::Object { name, key, handler } => Target {
                service: name,
                handler,
                key: Some(key),
                idempotency_key: None,
                headers: vec![],
            },
            RequestTarget::Workflow { name, key, handler } => Target {
                service: name,
                handler,
                key: Some(key),
                idempotency_key: None,
                headers: vec![],
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
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_state_get(key.to_owned()));

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(None)),
            Ok(Value::Success(mut s)) => {
                let t =
                    T::deserialize(&mut s).map_err(|e| Error::deserialization("get_state", e))?;
                Ok(Ok(Some(t)))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "get_state",
            }
            .into()),
            Err(e) => Err(e.into()),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn get_keys(&self) -> impl Future<Output = Result<Vec<String>, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_state_get_keys());

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(s)) => Ok(Ok(s)),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "get_keys",
            }
            .into()),
            Err(e) => Err(e.into()),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn set<T: Serialize>(&self, key: &str, t: T) {
        let mut inner_lock = must_lock!(self.inner);
        match t.serialize() {
            Ok(b) => {
                let _ = inner_lock.vm.sys_state_set(key.to_owned(), b);
            }
            Err(e) => {
                inner_lock.fail(Error::serialization("set_state", e));
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
        sleep_duration: Duration,
    ) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Duration since unix epoch cannot fail");
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(
            inner_lock,
            inner_lock.vm.sys_sleep(now + sleep_duration, Some(now))
        );

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(())),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "sleep",
            }
            .into()),
            Err(e) => Err(e.into()),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn request<Req, Res>(&self, request_target: RequestTarget, req: Req) -> Request<Req, Res> {
        Request::new(self, request_target, req)
    }

    pub fn call<Req: Serialize, Res: Deserialize>(
        &self,
        request_target: RequestTarget,
        idempotency_key: Option<String>,
        req: Req,
    ) -> impl CallFuture<Result<Res, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);

        let mut target: Target = request_target.into();
        target.idempotency_key = idempotency_key;
        let input = unwrap_or_trap!(
            inner_lock,
            Req::serialize(&req).map_err(|e| Error::serialization("call", e))
        );

        let call_handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_call(target, input));
        drop(inner_lock);

        // Let's prepare the two futures here
        let call_invocation_id_fut = InterceptErrorFuture::new(
            self.clone(),
            get_async_result(
                Arc::clone(&self.inner),
                call_handle.invocation_id_notification_handle,
            )
            .map(|res| match res {
                Ok(Value::Failure(f)) => Ok(Err(f.into())),
                Ok(Value::InvocationId(s)) => Ok(Ok(s)),
                Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: <&'static str>::from(v),
                    syscall: "call",
                }.into()),
                Err(e) => Err(e.into()),
            }),
        );
        let call_result_future = InterceptErrorFuture::new(
            self.clone(),
            get_async_result(
                Arc::clone(&self.inner),
                call_handle.call_notification_handle,
            )
            .map(|res| match res {
                Ok(Value::Success(mut s)) => {
                    let t =
                        Res::deserialize(&mut s).map_err(|e| Error::deserialization("call", e))?;
                    Ok(Ok(t))
                }
                Ok(Value::Failure(f)) => Ok(Err(f.into())),
                Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: <&'static str>::from(v),
                    syscall: "call",
                }
                .into()),
                Err(e) => Err(e.into()),
            }),
        );

        Either::Left(CallFutureImpl {
            invocation_id_future: call_invocation_id_fut.shared(),
            result_future: call_result_future,
            res: PhantomData,
            ctx: self.clone(),
        })
    }

    pub fn send<Req: Serialize>(
        &self,
        request_target: RequestTarget,
        idempotency_key: Option<String>,
        req: Req,
        delay: Option<Duration>,
    ) -> impl InvocationHandle {
        let mut inner_lock = must_lock!(self.inner);

        let mut target: Target = request_target.into();
        target.idempotency_key = idempotency_key;

        let input = match Req::serialize(&req) {
            Ok(b,) => b,
              Err(e) =>   {
                  inner_lock.fail(Error::serialization("call", e));
                  return Either::Right(TrapFuture::<()>::default())
              }
        };

        let send_handle = match
            inner_lock.vm.sys_send(
                target,
                input,
                delay.map(|delay| {
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .expect("Duration since unix epoch cannot fail")
                        + delay
                })
            ) {
            Ok(h,) => h,
            Err(e) =>   {
                inner_lock.fail(e.into());
                return Either::Right(TrapFuture::<()>::default())
            }
        };
        drop(inner_lock);

        let call_invocation_id_fut = InterceptErrorFuture::new(
            self.clone(),
            get_async_result(
                Arc::clone(&self.inner),
                send_handle.invocation_id_notification_handle,
            )
            .map(|res| match res {
                Ok(Value::Failure(f)) => Ok(Err(f.into())),
                Ok(Value::InvocationId(s)) => Ok(Ok(s)),
                Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: <&'static str>::from(v),
                    syscall: "call",
                }
                .into()),
                Err(e) => Err(e.into()),
            }),
        );

        Either::Left(SendRequestHandle {
            invocation_id_future: call_invocation_id_fut.shared(),
            ctx: self.clone(),
        })
    }

    pub fn invocation_handle(&self, invocation_id: String) -> impl InvocationHandle {
        InvocationIdBackedInvocationHandle {
            ctx: self.clone(),
            invocation_id,
        }
    }

    pub fn awakeable<T: Deserialize>(
        &self,
    ) -> (
        String,
        impl Future<Output = Result<T, TerminalError>> + Send,
    ) {
        let mut inner_lock = must_lock!(self.inner);
        let maybe_awakeable_id_and_handle = inner_lock.vm.sys_awakeable();

        let (awakeable_id, handle) = match maybe_awakeable_id_and_handle {
            Ok((s, handle)) => (s, handle),
            Err(e) => {
                inner_lock.fail(e.into());
                return (
                    // TODO NOW this is REALLY BAD. The reason for this is that we would need to return a future of a future instead, which is not nice.
                    //  we assume for the time being this works because no user should use the awakeable without doing any other syscall first, which will prevent this invalid awakeable id to work in the first place.
                    "invalid".to_owned(),
                    Either::Right(TrapFuture::default()),
                );
            }
        };
        drop(inner_lock);

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Success(mut s)) => {
                Ok(Ok(T::deserialize(&mut s).map_err(|e| Error::deserialization("awakeable", e))?))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "awakeable",
            }.into()),
            Err(e) => Err(e),
        });

        (
            awakeable_id,
            Either::Left(InterceptErrorFuture::new(self.clone(), poll_future)),
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
                inner_lock.fail(Error::serialization("resolve_awakeable", e));
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
    ) -> impl Future<Output = Result<T, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_get_promise(name.to_owned()));
        drop(inner_lock);

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Success(mut s)) => {
                let t = T::deserialize(&mut s).map_err(|e| Error::deserialization("promise", e))?;
                Ok(Ok(t))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "promise",
            }
            .into()),
            Err(e) => Err(e.into()),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn peek_promise<T: Deserialize>(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_peek_promise(name.to_owned()));
        drop(inner_lock);

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(None)),
            Ok(Value::Success(mut s)) => {
                let t = T::deserialize(&mut s)
                    .map_err(|e| Error::deserialization("peek_promise", e))?;
                Ok(Ok(Some(t)))
            }
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "peek_promise",
            }
            .into()),
            Err(e) => Err(e),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
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
    ) -> impl crate::context::RunFuture<Result<Out, TerminalError>> + Send + 'a
    where
        Run: RunClosure<Fut = Fut, Output = Out> + Send + 'a,
        Fut: Future<Output = HandlerResult<Out>> + Send + 'a,
        Out: Serialize + Deserialize + 'static,
    {
        let this = Arc::clone(&self.inner);

        InterceptErrorFuture::new(self.clone(), RunFuture::new(this, run_closure))
    }

    // Used by codegen
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
    Run: RunClosure<Fut = Fut, Output = Out> + Send,
    Fut: Future<Output = HandlerResult<Out>> + Send,
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
    Run: RunClosure<Fut = Fut, Output = Out> + Send,
    Out: Serialize + Deserialize,
    Fut: Future<Output = HandlerResult<Out>> + Send,
{
    type Output = Result<Result<Out, TerminalError>, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
        // let mut this = self.project();

        // loop {
        //     match this.state.as_mut().project() {
        //         RunStateProj::New => {
        //             let enter_result = {
        //                 must_lock!(this
        //                     .inner_ctx
        //                     .as_mut()
        //                     .expect("Future should not be polled after returning Poll::Ready"))
        //                 .vm
        //                 .sys_run_enter(this.name.to_owned())
        //             };
        //
        //             // Enter the side effect
        //             match enter_result.map_err(ErrorInner::VM)? {
        //                 RunEnterResult::Executed(NonEmptyValue::Success(mut v)) => {
        //                     let t = Out::deserialize(&mut v).map_err(|e| {
        //                         ErrorInner::Deserialization {
        //                             syscall: "run",
        //                             err: Box::new(e),
        //                         }
        //                     })?;
        //                     return Poll::Ready(Ok(Ok(t)));
        //                 }
        //                 RunEnterResult::Executed(NonEmptyValue::Failure(f)) => {
        //                     return Poll::Ready(Ok(Err(f.into())))
        //                 }
        //                 RunEnterResult::NotExecuted(_) => {}
        //             };
        //
        //             // We need to run the closure
        //             this.state.set(RunState::ClosureRunning {
        //                 start_time: Instant::now(),
        //                 fut: this
        //                     .closure
        //                     .take()
        //                     .expect("Future should not be polled after returning Poll::Ready")
        //                     .run(),
        //             });
        //         }
        //         RunStateProj::ClosureRunning { start_time, fut } => {
        //             let res = match ready!(fut.poll(cx)) {
        //                 Ok(t) => RunExitResult::Success(Out::serialize(&t).map_err(|e| {
        //                     ErrorInner::Serialization {
        //                         syscall: "run",
        //                         err: Box::new(e),
        //                     }
        //                 })?),
        //                 Err(e) => match e.0 {
        //                     HandlerErrorInner::Retryable(err) => RunExitResult::RetryableFailure {
        //                         attempt_duration: start_time.elapsed(),
        //                         failure: TerminalFailure {
        //                             code: 500,
        //                             message: err.to_string(),
        //                         },
        //                     },
        //                     HandlerErrorInner::Terminal(t) => {
        //                         RunExitResult::TerminalFailure(TerminalError(t).into())
        //                     }
        //                 },
        //             };
        //
        //             let inner_ctx = this
        //                 .inner_ctx
        //                 .take()
        //                 .expect("Future should not be polled after returning Poll::Ready");
        //
        //             let handle = {
        //                 must_lock!(inner_ctx)
        //                     .vm
        //                     .sys_run_exit(res, mem::take(this.retry_policy))
        //             };
        //
        //             this.state.set(RunState::PollFutureRunning {
        //                 fut: VmAsyncResultPollFuture::maybe_new(Cow::Owned(inner_ctx), handle),
        //             });
        //         }
        //         RunStateProj::PollFutureRunning { fut } => {
        //             let value = ready!(fut.poll(cx))?;
        //
        //             return Poll::Ready(match value {
        //                 Value::Void => Err(ErrorInner::UnexpectedValueVariantForSyscall {
        //                     variant: "empty",
        //                     syscall: "run",
        //                 }
        //                 .into()),
        //                 Value::Success(mut s) => {
        //                     let t = Out::deserialize(&mut s).map_err(|e| {
        //                         ErrorInner::Deserialization {
        //                             syscall: "run",
        //                             err: Box::new(e),
        //                         }
        //                     })?;
        //                     Ok(Ok(t))
        //                 }
        //                 Value::Failure(f) => Ok(Err(f.into())),
        //                 Value::StateKeys(_) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
        //                     variant: "state_keys",
        //                     syscall: "run",
        //                 }
        //                 .into()),
        //                 Value::InvocationId(_) => {
        //                     Err(ErrorInner::UnexpectedValueVariantForSyscall {
        //                         variant: "invocation_id",
        //                         syscall: "run",
        //                     }
        //                     .into())
        //                 }
        //             });
        //         }
        //     }
        // }
    }
}

struct SendRequestHandle<InvIdFut: Future> {
    invocation_id_future: Shared<InvIdFut>,
    ctx: ContextInternal,
}

impl<InvIdFut: Future<Output = Result<String, TerminalError>> + Send> InvocationHandle
    for SendRequestHandle<InvIdFut>
{
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        async move { cloned_invocation_id_fut.await }
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        let cloned_ctx = Arc::clone(&self.ctx.inner);
        async move {
            let inv_id = cloned_invocation_id_fut.await?;
            let mut inner_lock = must_lock!(cloned_ctx);
            let _ = inner_lock.vm.sys_cancel_invocation(inv_id);
            drop(inner_lock);
            Ok(())
        }
    }
}

pin_project! {
    struct CallFutureImpl<InvIdFut: Future, ResultFut, Res> {
       #[pin]
        invocation_id_future: Shared<InvIdFut>,
        #[pin]
        result_future: ResultFut,
        res: PhantomData<fn() -> Res>,
        ctx: ContextInternal,
    }
}

impl<InvIdFut: Future, ResultFut: Future<Output = Result<Res, TerminalError>> + Send, Res> Future
    for CallFutureImpl<InvIdFut, ResultFut, Res>
{
    type Output = ResultFut::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.result_future.poll(cx)
    }
}

impl<InvIdFut: Future<Output = Result<String, TerminalError>> + Send, ResultFut, Res>
    InvocationHandle for CallFutureImpl<InvIdFut, ResultFut, Res>
{
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        async move { cloned_invocation_id_fut.await }
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        let cloned_ctx = Arc::clone(&self.ctx.inner);
        async move {
            let inv_id = cloned_invocation_id_fut.await?;
            let mut inner_lock = must_lock!(cloned_ctx);
            let _ = inner_lock.vm.sys_cancel_invocation(inv_id);
            drop(inner_lock);
            Ok(())
        }
    }
}

impl<A: InvocationHandle, B: InvocationHandle> InvocationHandle for Either<A, B> {
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        match self {
            Either::Left(l) => Either::Left(l.invocation_id()),
            Either::Right(r) => Either::Right(r.invocation_id()),
        }
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        match self {
            Either::Left(l) => Either::Left(l.cancel()),
            Either::Right(r) => Either::Right(r.cancel()),
        }
    }
}

impl<A, B, O> CallFuture<O> for Either<A, B>
where
    A: CallFuture<O>,
    B: CallFuture<O>,
{


}

struct InvocationIdBackedInvocationHandle {
    ctx: ContextInternal,
    invocation_id: String,
}

impl InvocationHandle for InvocationIdBackedInvocationHandle {
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        ready(Ok(self.invocation_id.clone()))
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.ctx.inner);
        let _ = inner_lock
            .vm
            .sys_cancel_invocation(self.invocation_id.clone());
        ready(Ok(()))
    }
}

impl Error {
    fn serialization<E: std::error::Error + Send + Sync + 'static>(
        syscall: &'static str,
        e: E,
    ) -> Self {
        ErrorInner::Serialization {
            syscall,
            err: Box::new(e),
        }
        .into()
    }

    fn deserialization<E: std::error::Error + Send + Sync + 'static>(
        syscall: &'static str,
        e: E,
    ) -> Self {
        ErrorInner::Deserialization {
            syscall,
            err: Box::new(e),
        }
        .into()
    }
}

fn get_async_result(
    ctx: Arc<Mutex<ContextInternalInner>>,
    handle: NotificationHandle,
) -> impl Future<Output = Result<Value, Error>> + Send {
    async move {
        loop {
            let mut inner_lock = must_lock!(ctx);

            // Let's consume some output to begin with
            let out = inner_lock.vm.take_output();
            match out {
                TakeOutputResult::Buffer(b) => {
                    if !inner_lock.write.send(b) {
                        return Err(ErrorInner::Suspended);
                    }
                }
                TakeOutputResult::EOF => return Err(ErrorInner::UnexpectedOutputClosed),
            }

            // Let's do some progress now
            match inner_lock.vm.do_progress(vec![handle]) {
                Ok(DoProgressResponse::AnyCompleted) => {
                    // We're good, we got the response
                    break;
                }
                Ok(DoProgressResponse::ReadFromInput) => {
                    drop(inner_lock);
                    match inner_lock.read.recv().await {
                        Some(Ok(b)) => must_lock!(ctx).vm.notify_input(b),
                        Some(Err(e)) => must_lock!(ctx).vm.notify_error(
                            CoreError::new(500u16, format!("Error when reading the body {e:?}",)),
                            None,
                        ),
                        None => must_lock!(ctx).vm.notify_input_closed(),
                    }
                    continue;
                }
                Ok(DoProgressResponse::ExecuteRun(_)) => {
                    unimplemented!()
                }
                Ok(DoProgressResponse::WaitingPendingRun) => {
                    unimplemented!()
                }
                Ok(DoProgressResponse::CancelSignalReceived) => {
                    unimplemented!()
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
        }
        let mut inner_lock = must_lock!(ctx);

        // At this point let's try to take the notification
        match inner_lock.vm.take_notification(handle) {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {
                panic!("This is not supposed to happen, handle was flagged as completed")
            }
            Err(e) => return Err(e.into()),
        }
    }.map_err(Error::from)
}
