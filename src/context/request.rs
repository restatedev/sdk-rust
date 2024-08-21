use crate::endpoint::ContextInternal;
use crate::errors::TerminalError;
use crate::serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

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

pub struct Request<'a, Req, Res> {
    ctx: &'a ContextInternal,
    request_target: RequestTarget,
    req: Req,
    res: PhantomData<Res>,
}

impl<'a, Req, Res> Request<'a, Req, Res> {
    pub(crate) fn new(ctx: &'a ContextInternal, request_target: RequestTarget, req: Req) -> Self {
        Self {
            ctx,
            request_target,
            req,
            res: PhantomData,
        }
    }

    pub fn call(self) -> impl Future<Output = Result<Res, TerminalError>> + Send
    where
        Req: Serialize + 'static,
        Res: Deserialize + 'static,
    {
        self.ctx.call(self.request_target, self.req)
    }

    pub fn send(self)
    where
        Req: Serialize + 'static,
    {
        self.ctx.send(self.request_target, self.req, None)
    }

    pub fn send_with_delay(self, duration: Duration)
    where
        Req: Serialize + 'static,
    {
        self.ctx.send(self.request_target, self.req, Some(duration))
    }
}
