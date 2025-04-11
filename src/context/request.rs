use super::DurableFuture;

use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use crate::serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
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
    pub fn send(self) -> impl InvocationHandle
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
    pub fn send_after(self, delay: Duration) -> impl InvocationHandle
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

pub trait InvocationHandle {
    fn invocation_id(&self) -> impl Future<Output = Result<String, TerminalError>> + Send;
    fn cancel(&self) -> impl Future<Output = Result<(), TerminalError>> + Send;
}

pub trait CallFuture:
    DurableFuture<Output = Result<Self::Response, TerminalError>> + InvocationHandle
{
    type Response;
}
