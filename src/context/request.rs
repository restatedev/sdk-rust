use super::DurableFuture;

use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use crate::serde::{Deserialize, Serialize};
use futures::FutureExt;
use futures::future::BoxFuture;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::time::Duration;

/// Target of a request to a Restate service.
#[derive(Debug, Clone)]
pub enum RequestTarget {
    Service {
        name: String,
        handler: String,
    },
    Object {
        name: String,
        key: String,
        handler: String,
    },
    Workflow {
        name: String,
        key: String,
        handler: String,
    },
}

impl RequestTarget {
    pub fn service(name: impl Into<String>, handler: impl Into<String>) -> Self {
        Self::Service {
            name: name.into(),
            handler: handler.into(),
        }
    }

    pub fn object(
        name: impl Into<String>,
        key: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        Self::Object {
            name: name.into(),
            key: key.into(),
            handler: handler.into(),
        }
    }

    pub fn workflow(
        name: impl Into<String>,
        key: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        Self::Workflow {
            name: name.into(),
            key: key.into(),
            handler: handler.into(),
        }
    }
}

impl fmt::Display for RequestTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestTarget::Service { name, handler } => write!(f, "{name}/{handler}"),
            RequestTarget::Object { name, key, handler } => write!(f, "{name}/{key}/{handler}"),
            RequestTarget::Workflow { name, key, handler } => write!(f, "{name}/{key}/{handler}"),
        }
    }
}

/// This struct encapsulates the parameters for a request to a service.
pub struct Request<'a, Req, Res = ()> {
    ctx: &'a ContextInternal,
    request_target: RequestTarget,
    idempotency_key: Option<String>,
    headers: Vec<(String, String)>,
    req: Req,
    res: PhantomData<Res>,
}

impl<'a, Req, Res> Request<'a, Req, Res> {
    pub(crate) fn new(ctx: &'a ContextInternal, request_target: RequestTarget, req: Req) -> Self {
        Self {
            ctx,
            request_target,
            idempotency_key: None,
            headers: vec![],
            req,
            res: PhantomData,
        }
    }

    pub fn header(mut self, key: String, value: String) -> Self {
        self.headers.push((key, value));
        self
    }

    /// Add idempotency key to the request
    pub fn idempotency_key(mut self, idempotency_key: impl Into<String>) -> Self {
        self.idempotency_key = Some(idempotency_key.into());
        self
    }

    /// Call a service. This returns a future encapsulating the response.
    pub fn call(self) -> impl CallFuture<Response = Res> + Send
    where
        Req: Serialize + 'static,
        Res: Deserialize + 'static,
    {
        self.ctx.call(
            self.request_target,
            self.idempotency_key,
            self.headers,
            self.req,
        )
    }

    /// Send the request to the service, without waiting for the response.
    ///
    /// The request is sent eagerly; you can drop the returned [`SendHandle`] to fire-and-forget,
    /// or `.await` it to obtain an [`InvocationHandle`] to the newly created invocation.
    pub fn send(self) -> SendHandle
    where
        Req: Serialize + 'static,
    {
        self.ctx.send(
            self.request_target,
            self.idempotency_key,
            self.headers,
            self.req,
            None,
        )
    }

    /// Schedule the request to the service, without waiting for the response.
    ///
    /// Same as [`Request::send`], but the invocation is executed after the given `delay`.
    pub fn send_after(self, delay: Duration) -> SendHandle
    where
        Req: Serialize + 'static,
    {
        self.ctx.send(
            self.request_target,
            self.idempotency_key,
            self.headers,
            self.req,
            Some(delay),
        )
    }
}

/// A handle to an invocation.
///
/// You can obtain an [`InvocationHandle`] in three ways:
///
/// * From [`ContextClient::invocation_handle`][crate::context::ContextClient::invocation_handle],
///   when you already know the invocation id.
/// * By `.await`-ing the [`SendHandle`] returned by [`Request::send`]/[`Request::send_after`].
/// * By `.await`-ing [`CallFuture::invocation_handle`].
///
/// Once obtained, the invocation id is known synchronously via [`InvocationHandle::invocation_id`].
pub struct InvocationHandle {
    ctx: ContextInternal,
    invocation_id: String,
}

impl InvocationHandle {
    pub(crate) fn new(ctx: ContextInternal, invocation_id: String) -> Self {
        Self { ctx, invocation_id }
    }

    /// The invocation id of the target invocation.
    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    /// Cancel the invocation.
    pub fn cancel(&self) {
        self.ctx.cancel_invocation(&self.invocation_id)
    }

    /// Attach to the invocation, returning a future that resolves with its output/result.
    pub fn attach<T: Deserialize + 'static>(
        &self,
    ) -> impl DurableFuture<Output = Result<T, TerminalError>> + Send + use<T> {
        self.ctx.attach_invocation(self.invocation_id.clone())
    }

    /// Get a handle to a named [signal][crate::context::ContextSignals] on this invocation,
    /// which you can [resolve][SignalHandle::resolve] or [reject][SignalHandle::reject].
    pub fn signal(&self, name: impl Into<String>) -> SignalHandle {
        SignalHandle {
            ctx: self.ctx.clone(),
            invocation_id: self.invocation_id.clone(),
            name: name.into(),
        }
    }
}

/// A handle to a named signal on a target invocation, obtained via [`InvocationHandle::signal`].
///
/// Use it to complete a signal the target invocation is (or will be) awaiting via
/// [`ContextSignals::signal`][crate::context::ContextSignals::signal].
pub struct SignalHandle {
    ctx: ContextInternal,
    invocation_id: String,
    name: String,
}

impl SignalHandle {
    /// Resolve the signal with the given value.
    pub fn resolve<T: Serialize + 'static>(self, value: T) {
        self.ctx
            .resolve_signal(&self.invocation_id, &self.name, value)
    }

    /// Reject the signal. The awaiting handler observes a terminal error.
    pub fn reject(self, failure: TerminalError) {
        self.ctx
            .reject_signal(&self.invocation_id, &self.name, failure)
    }
}

/// Handle returned by [`Request::send`]/[`Request::send_after`].
///
/// The request is already sent; `.await` this handle to obtain an [`InvocationHandle`] to the
/// created invocation, or drop it to fire-and-forget.
pub struct SendHandle {
    invocation_id_future: BoxFuture<'static, Result<String, TerminalError>>,
    ctx: ContextInternal,
}

impl SendHandle {
    pub(crate) fn new(
        ctx: ContextInternal,
        invocation_id_future: BoxFuture<'static, Result<String, TerminalError>>,
    ) -> Self {
        Self {
            invocation_id_future,
            ctx,
        }
    }
}

impl IntoFuture for SendHandle {
    type Output = Result<InvocationHandle, TerminalError>;
    type IntoFuture = BoxFuture<'static, Result<InvocationHandle, TerminalError>>;

    fn into_future(self) -> Self::IntoFuture {
        let ctx = self.ctx;
        async move {
            let invocation_id = self.invocation_id_future.await?;
            Ok(InvocationHandle::new(ctx, invocation_id))
        }
        .boxed()
    }
}

pub trait CallFuture: DurableFuture<Output = Result<Self::Response, TerminalError>> {
    type Response;

    /// Returns a future that resolves with an [`InvocationHandle`] to this call's invocation,
    /// without consuming or awaiting the response.
    fn invocation_handle(
        &self,
    ) -> impl Future<Output = Result<InvocationHandle, TerminalError>> + Send;

    /// Returns the invocation id of this call.
    #[deprecated(
        since = "0.11.0",
        note = "use `invocation_handle().await?.invocation_id()` instead"
    )]
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send;
}
