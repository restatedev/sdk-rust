use crate::context::{
    CallFuture, DurableFuture, InvocationHandle, Request, RequestTarget, RunClosure, RunFuture,
    RunRetryPolicy,
};
use crate::endpoint::futures::async_result_poll::VmAsyncResultPollFuture;
use crate::endpoint::futures::durable_future_impl::DurableFutureImpl;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::endpoint::futures::select_poll::VmSelectAsyncResultPollFuture;
use crate::endpoint::futures::trap::TrapFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::endpoint::{Error, ErrorInner, InputReceiver, OutputSender};
use crate::errors::{HandlerErrorInner, HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use futures::future::{BoxFuture, Either, Shared};
use futures::{FutureExt, TryFutureExt};
use pin_project_lite::pin_project;
use restate_sdk_shared_core::{
    CoreVM, DoProgressResponse, Error as CoreError, Header, NonEmptyValue, NotificationHandle,
    RetryPolicy, RunExitResult, TakeOutputResult, Target, TerminalFailure, Value, VM,
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

    /// We remember here the state of the span replaying field state, because setting it might be expensive (it's guarded behind locks and other stuff).
    /// For details, see [ContextInternalInner::maybe_flip_span_replaying_field]
    pub(super) span_replaying_field_state: bool,
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
            span_replaying_field_state: false,
        }
    }

    pub(super) fn fail(&mut self, e: Error) {
        self.maybe_flip_span_replaying_field();
        self.vm.notify_error(
            CoreError::new(500u16, e.0.to_string())
                .with_stacktrace(Cow::Owned(format!("{:#}", e.0))),
            None,
        );
        self.handler_state.mark_error(e);
    }

    pub(super) fn maybe_flip_span_replaying_field(&mut self) {
        if !self.span_replaying_field_state && self.vm.is_replaying() {
            tracing::Span::current().record("restate.sdk.is_replaying", true);
            self.span_replaying_field_state = true;
        } else if self.span_replaying_field_state && !self.vm.is_replaying() {
            tracing::Span::current().record("restate.sdk.is_replaying", false);
            self.span_replaying_field_state = false;
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

macro_rules! unwrap_or_trap_durable_future {
    ($ctx:expr, $inner_lock:expr, $res:expr) => {
        match $res {
            Ok(t) => t,
            Err(e) => {
                $inner_lock.fail(e.into());
                return DurableFutureImpl::new(
                    $ctx.clone(),
                    NotificationHandle::from(u32::MAX),
                    Either::Right(TrapFuture::default()),
                );
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
        inner_lock.maybe_flip_span_replaying_field();

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
        inner_lock.maybe_flip_span_replaying_field();

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
            Err(e) => Err(e),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn get_keys(&self) -> impl Future<Output = Result<Vec<String>, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_state_get_keys());
        inner_lock.maybe_flip_span_replaying_field();

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(Value::StateKeys(s)) => Ok(Ok(s)),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "get_keys",
            }
            .into()),
            Err(e) => Err(e),
        });

        Either::Left(InterceptErrorFuture::new(self.clone(), poll_future))
    }

    pub fn set<T: Serialize>(&self, key: &str, t: T) {
        let mut inner_lock = must_lock!(self.inner);
        match t.serialize() {
            Ok(b) => {
                let _ = inner_lock.vm.sys_state_set(key.to_owned(), b);
                inner_lock.maybe_flip_span_replaying_field();
            }
            Err(e) => {
                inner_lock.fail(Error::serialization("set_state", e));
            }
        }
    }

    pub fn clear(&self, key: &str) {
        let mut inner_lock = must_lock!(self.inner);
        let _ = inner_lock.vm.sys_state_clear(key.to_string());
        inner_lock.maybe_flip_span_replaying_field();
    }

    pub fn clear_all(&self) {
        let mut inner_lock = must_lock!(self.inner);
        let _ = inner_lock.vm.sys_state_clear_all();
        inner_lock.maybe_flip_span_replaying_field();
    }

    pub fn select(
        &self,
        handles: Vec<NotificationHandle>,
    ) -> impl Future<Output = Result<usize, TerminalError>> + Send {
        InterceptErrorFuture::new(
            self.clone(),
            VmSelectAsyncResultPollFuture::new(self.inner.clone(), handles).map_err(Error::from),
        )
    }

    pub fn sleep(
        &self,
        sleep_duration: Duration,
    ) -> impl DurableFuture<Output = Result<(), TerminalError>> + Send {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Duration since unix epoch cannot fail");
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap_durable_future!(
            self,
            inner_lock,
            inner_lock
                .vm
                .sys_sleep(String::default(), now + sleep_duration, Some(now))
        );
        inner_lock.maybe_flip_span_replaying_field();

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Void) => Ok(Ok(())),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "sleep",
            }
            .into()),
            Err(e) => Err(e),
        });

        DurableFutureImpl::new(self.clone(), handle, Either::Left(poll_future))
    }

    pub fn request<Req, Res>(&self, request_target: RequestTarget, req: Req) -> Request<Req, Res> {
        Request::new(self, request_target, req)
    }

    pub fn call<Req: Serialize, Res: Deserialize>(
        &self,
        request_target: RequestTarget,
        idempotency_key: Option<String>,
        headers: Vec<(String, String)>,
        req: Req,
    ) -> impl CallFuture<Response = Res> + Send {
        let mut inner_lock = must_lock!(self.inner);

        let mut target: Target = request_target.into();
        target.idempotency_key = idempotency_key;
        target.headers = headers
            .into_iter()
            .map(|(k, v)| Header {
                key: k.into(),
                value: v.into(),
            })
            .collect();
        let call_result = Req::serialize(&req)
            .map_err(|e| Error::serialization("call", e))
            .and_then(|input| inner_lock.vm.sys_call(target, input).map_err(Into::into));

        let call_handle = match call_result {
            Ok(t) => t,
            Err(e) => {
                inner_lock.fail(e);
                return CallFutureImpl {
                    invocation_id_future: Either::Right(TrapFuture::default()).shared(),
                    result_future: Either::Right(TrapFuture::default()),
                    call_notification_handle: NotificationHandle::from(u32::MAX),
                    ctx: self.clone(),
                };
            }
        };
        inner_lock.maybe_flip_span_replaying_field();
        drop(inner_lock);

        // Let's prepare the two futures here
        let invocation_id_fut = InterceptErrorFuture::new(
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
                }
                .into()),
                Err(e) => Err(e),
            }),
        );
        let result_future = get_async_result(
            Arc::clone(&self.inner),
            call_handle.call_notification_handle,
        )
        .map(|res| match res {
            Ok(Value::Success(mut s)) => Ok(Ok(
                Res::deserialize(&mut s).map_err(|e| Error::deserialization("call", e))?
            )),
            Ok(Value::Failure(f)) => Ok(Err(TerminalError::from(f))),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "call",
            }
            .into()),
            Err(e) => Err(e),
        });

        CallFutureImpl {
            invocation_id_future: Either::Left(invocation_id_fut).shared(),
            result_future: Either::Left(result_future),
            call_notification_handle: call_handle.call_notification_handle,
            ctx: self.clone(),
        }
    }

    pub fn send<Req: Serialize>(
        &self,
        request_target: RequestTarget,
        idempotency_key: Option<String>,
        headers: Vec<(String, String)>,
        req: Req,
        delay: Option<Duration>,
    ) -> impl InvocationHandle {
        let mut inner_lock = must_lock!(self.inner);

        let mut target: Target = request_target.into();
        target.idempotency_key = idempotency_key;
        target.headers = headers
            .into_iter()
            .map(|(k, v)| Header {
                key: k.into(),
                value: v.into(),
            })
            .collect();
        let input = match Req::serialize(&req) {
            Ok(b) => b,
            Err(e) => {
                inner_lock.fail(Error::serialization("call", e));
                return Either::Right(TrapFuture::<()>::default());
            }
        };

        let send_handle = match inner_lock.vm.sys_send(
            target,
            input,
            delay.map(|delay| {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("Duration since unix epoch cannot fail")
                    + delay
            }),
        ) {
            Ok(h) => h,
            Err(e) => {
                inner_lock.fail(e.into());
                return Either::Right(TrapFuture::<()>::default());
            }
        };
        inner_lock.maybe_flip_span_replaying_field();
        drop(inner_lock);

        let invocation_id_fut = InterceptErrorFuture::new(
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
                Err(e) => Err(e),
            }),
        );

        Either::Left(SendRequestHandle {
            invocation_id_future: invocation_id_fut.shared(),
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
        impl DurableFuture<Output = Result<T, TerminalError>> + Send,
    ) {
        let mut inner_lock = must_lock!(self.inner);
        let maybe_awakeable_id_and_handle = inner_lock.vm.sys_awakeable();
        inner_lock.maybe_flip_span_replaying_field();

        let (awakeable_id, handle) = match maybe_awakeable_id_and_handle {
            Ok((s, handle)) => (s, handle),
            Err(e) => {
                inner_lock.fail(e.into());
                return (
                    // TODO NOW this is REALLY BAD. The reason for this is that we would need to return a future of a future instead, which is not nice.
                    //  we assume for the time being this works because no user should use the awakeable without doing any other syscall first, which will prevent this invalid awakeable id to work in the first place.
                    "invalid".to_owned(),
                    DurableFutureImpl::new(
                        self.clone(),
                        NotificationHandle::from(u32::MAX),
                        Either::Right(TrapFuture::default()),
                    ),
                );
            }
        };
        drop(inner_lock);

        let poll_future = get_async_result(Arc::clone(&self.inner), handle).map(|res| match res {
            Ok(Value::Success(mut s)) => Ok(Ok(
                T::deserialize(&mut s).map_err(|e| Error::deserialization("awakeable", e))?
            )),
            Ok(Value::Failure(f)) => Ok(Err(f.into())),
            Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                variant: <&'static str>::from(v),
                syscall: "awakeable",
            }
            .into()),
            Err(e) => Err(e),
        });

        (
            awakeable_id,
            DurableFutureImpl::new(self.clone(), handle, Either::Left(poll_future)),
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
    ) -> impl DurableFuture<Output = Result<T, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap_durable_future!(
            self,
            inner_lock,
            inner_lock.vm.sys_get_promise(name.to_owned())
        );
        inner_lock.maybe_flip_span_replaying_field();
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
            Err(e) => Err(e),
        });

        DurableFutureImpl::new(self.clone(), handle, Either::Left(poll_future))
    }

    pub fn peek_promise<T: Deserialize>(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + Send {
        let mut inner_lock = must_lock!(self.inner);
        let handle = unwrap_or_trap!(inner_lock, inner_lock.vm.sys_peek_promise(name.to_owned()));
        inner_lock.maybe_flip_span_replaying_field();
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
    ) -> impl RunFuture<Result<Out, TerminalError>> + Send + 'a
    where
        Run: RunClosure<Fut = Fut, Output = Out> + Send + 'a,
        Fut: Future<Output = HandlerResult<Out>> + Send + 'a,
        Out: Serialize + Deserialize + 'static,
    {
        let this = Arc::clone(&self.inner);
        InterceptErrorFuture::new(self.clone(), RunFutureImpl::new(this, run_closure))
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
        inner_lock.maybe_flip_span_replaying_field();
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
    struct RunFutureImpl<Run, Ret, RunFnFut> {
        name: String,
        retry_policy: RetryPolicy,
        phantom_data: PhantomData<fn() -> Ret>,
        #[pin]
        state: RunState<Run, RunFnFut, Ret>,
    }
}

pin_project! {
    #[project = RunStateProj]
    enum RunState<Run, RunFnFut, Ret> {
        New {
            ctx: Option<Arc<Mutex<ContextInternalInner>>>,
            closure: Option<Run>,
        },
        ClosureRunning {
            ctx: Option<Arc<Mutex<ContextInternalInner>>>,
            handle: NotificationHandle,
            start_time: Instant,
            #[pin]
            closure_fut: RunFnFut,
        },
        WaitingResultFut {
            result_fut: BoxFuture<'static, Result<Result<Ret, TerminalError>, Error>>
        }
    }
}

impl<Run, Ret, RunFnFut> RunFutureImpl<Run, Ret, RunFnFut> {
    fn new(ctx: Arc<Mutex<ContextInternalInner>>, closure: Run) -> Self {
        Self {
            name: "".to_string(),
            retry_policy: RetryPolicy::Infinite,
            phantom_data: PhantomData,
            state: RunState::New {
                ctx: Some(ctx),
                closure: Some(closure),
            },
        }
    }

    fn boxed_result_fut(
        ctx: Arc<Mutex<ContextInternalInner>>,
        handle: NotificationHandle,
    ) -> BoxFuture<'static, Result<Result<Ret, TerminalError>, Error>>
    where
        Ret: Deserialize,
    {
        get_async_result(Arc::clone(&ctx), handle)
            .map(|res| match res {
                Ok(Value::Success(mut s)) => {
                    let t =
                        Ret::deserialize(&mut s).map_err(|e| Error::deserialization("run", e))?;
                    Ok(Ok(t))
                }
                Ok(Value::Failure(f)) => Ok(Err(f.into())),
                Ok(v) => Err(ErrorInner::UnexpectedValueVariantForSyscall {
                    variant: <&'static str>::from(v),
                    syscall: "run",
                }
                .into()),
                Err(e) => Err(e),
            })
            .boxed()
    }
}

impl<Run, Ret, RunFnFut> RunFuture<Result<Result<Ret, TerminalError>, Error>>
    for RunFutureImpl<Run, Ret, RunFnFut>
where
    Run: RunClosure<Fut = RunFnFut, Output = Ret> + Send,
    Ret: Serialize + Deserialize,
    RunFnFut: Future<Output = HandlerResult<Ret>> + Send,
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

impl<Run, Ret, RunFnFut> Future for RunFutureImpl<Run, Ret, RunFnFut>
where
    Run: RunClosure<Fut = RunFnFut, Output = Ret> + Send,
    Ret: Serialize + Deserialize,
    RunFnFut: Future<Output = HandlerResult<Ret>> + Send,
{
    type Output = Result<Result<Ret, TerminalError>, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.state.as_mut().project() {
                RunStateProj::New { ctx, closure, .. } => {
                    let ctx = ctx
                        .take()
                        .expect("Future should not be polled after returning Poll::Ready");
                    let closure = closure
                        .take()
                        .expect("Future should not be polled after returning Poll::Ready");
                    let mut inner_ctx = must_lock!(ctx);

                    let handle = inner_ctx
                        .vm
                        .sys_run(this.name.to_owned())
                        .map_err(ErrorInner::from)?;

                    // Now we do progress once to check whether this closure should be executed or not.
                    match inner_ctx.vm.do_progress(vec![handle]) {
                        Ok(DoProgressResponse::ExecuteRun(handle_to_run)) => {
                            // In case it returns ExecuteRun, it must be the handle we just gave it,
                            // and it means we need to execute the closure
                            assert_eq!(handle, handle_to_run);

                            drop(inner_ctx);
                            this.state.set(RunState::ClosureRunning {
                                ctx: Some(ctx),
                                handle,
                                start_time: Instant::now(),
                                closure_fut: closure.run(),
                            });
                        }
                        Ok(DoProgressResponse::CancelSignalReceived) => {
                            drop(inner_ctx);
                            // Got cancellation!
                            this.state.set(RunState::WaitingResultFut {
                                result_fut: async {
                                    Ok(Err(TerminalError::from(TerminalFailure {
                                        code: 409,
                                        message: "cancelled".to_string(),
                                    })))
                                }
                                .boxed(),
                            })
                        }
                        _ => {
                            drop(inner_ctx);
                            // In all the other cases, just move on waiting the result,
                            // the poll future state will take care of doing whatever needs to be done here,
                            // that is propagating state machine error, or result, or whatever
                            this.state.set(RunState::WaitingResultFut {
                                result_fut: Self::boxed_result_fut(Arc::clone(&ctx), handle),
                            })
                        }
                    }
                }
                RunStateProj::ClosureRunning {
                    ctx,
                    handle,
                    start_time,
                    closure_fut,
                } => {
                    let res = match ready!(closure_fut.poll(cx)) {
                        Ok(t) => RunExitResult::Success(Ret::serialize(&t).map_err(|e| {
                            ErrorInner::Serialization {
                                syscall: "run",
                                err: Box::new(e),
                            }
                        })?),
                        Err(e) => match e.0 {
                            HandlerErrorInner::Retryable(err) => RunExitResult::RetryableFailure {
                                attempt_duration: start_time.elapsed(),
                                error: CoreError::new(500u16, err.to_string()),
                            },
                            HandlerErrorInner::Terminal(t) => {
                                RunExitResult::TerminalFailure(TerminalError(t).into())
                            }
                        },
                    };

                    let ctx = ctx
                        .take()
                        .expect("Future should not be polled after returning Poll::Ready");
                    let handle = *handle;

                    let _ = {
                        must_lock!(ctx).vm.propose_run_completion(
                            handle,
                            res,
                            mem::take(this.retry_policy),
                        )
                    };

                    this.state.set(RunState::WaitingResultFut {
                        result_fut: Self::boxed_result_fut(Arc::clone(&ctx), handle),
                    });
                }
                RunStateProj::WaitingResultFut { result_fut } => return result_fut.poll_unpin(cx),
            }
        }
    }
}

pin_project! {
    struct CallFutureImpl<InvIdFut: Future, ResultFut> {
        #[pin]
        invocation_id_future: Shared<InvIdFut>,
        #[pin]
        result_future: ResultFut,
        call_notification_handle: NotificationHandle,
        ctx: ContextInternal,
    }
}

impl<InvIdFut, ResultFut, Res> Future for CallFutureImpl<InvIdFut, ResultFut>
where
    InvIdFut: Future<Output = Result<String, TerminalError>> + Send,
    ResultFut: Future<Output = Result<Result<Res, TerminalError>, Error>> + Send,
{
    type Output = Result<Res, TerminalError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let result = ready!(this.result_future.poll(cx));

        match result {
            Ok(r) => Poll::Ready(r),
            Err(e) => {
                this.ctx.fail(e);

                // Here is the secret sauce. This will immediately cause the whole future chain to be polled,
                //  but the poll here will be intercepted by HandlerStateAwareFuture
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

impl<InvIdFut, ResultFut> InvocationHandle for CallFutureImpl<InvIdFut, ResultFut>
where
    InvIdFut: Future<Output = Result<String, TerminalError>> + Send,
{
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        Shared::clone(&self.invocation_id_future)
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        let cloned_ctx = Arc::clone(&self.ctx.inner);
        async move {
            let inv_id = cloned_invocation_id_fut.await?;
            let mut inner_lock = must_lock!(cloned_ctx);
            let _ = inner_lock.vm.sys_cancel_invocation(inv_id);
            inner_lock.maybe_flip_span_replaying_field();
            drop(inner_lock);
            Ok(())
        }
    }
}

impl<InvIdFut, ResultFut, Res> CallFuture for CallFutureImpl<InvIdFut, ResultFut>
where
    InvIdFut: Future<Output = Result<String, TerminalError>> + Send,
    ResultFut: Future<Output = Result<Result<Res, TerminalError>, Error>> + Send,
{
    type Response = Res;
}

impl<InvIdFut, ResultFut> crate::context::macro_support::SealedDurableFuture
    for CallFutureImpl<InvIdFut, ResultFut>
where
    InvIdFut: Future,
{
    fn inner_context(&self) -> ContextInternal {
        self.ctx.clone()
    }

    fn handle(&self) -> NotificationHandle {
        self.call_notification_handle
    }
}

impl<InvIdFut, ResultFut, Res> DurableFuture for CallFutureImpl<InvIdFut, ResultFut>
where
    InvIdFut: Future<Output = Result<String, TerminalError>> + Send,
    ResultFut: Future<Output = Result<Result<Res, TerminalError>, Error>> + Send,
{
}

struct SendRequestHandle<InvIdFut: Future> {
    invocation_id_future: Shared<InvIdFut>,
    ctx: ContextInternal,
}

impl<InvIdFut: Future<Output = Result<String, TerminalError>> + Send> InvocationHandle
    for SendRequestHandle<InvIdFut>
{
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send {
        Shared::clone(&self.invocation_id_future)
    }

    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send {
        let cloned_invocation_id_fut = Shared::clone(&self.invocation_id_future);
        let cloned_ctx = Arc::clone(&self.ctx.inner);
        async move {
            let inv_id = cloned_invocation_id_fut.await?;
            let mut inner_lock = must_lock!(cloned_ctx);
            let _ = inner_lock.vm.sys_cancel_invocation(inv_id);
            inner_lock.maybe_flip_span_replaying_field();
            drop(inner_lock);
            Ok(())
        }
    }
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

impl<A, B> InvocationHandle for Either<A, B>
where
    A: InvocationHandle,
    B: InvocationHandle,
{
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
    VmAsyncResultPollFuture::new(ctx, handle).map_err(Error::from)
}
