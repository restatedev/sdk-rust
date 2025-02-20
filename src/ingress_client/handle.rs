use std::{
    fmt::{self, Display, Formatter},
    marker::PhantomData,
    time::Duration,
};

use super::internal::{IngressClientError, IngressInternal};
use crate::serde::Deserialize;

/// The target invocation or workflow to retrieve the handle of.
#[derive(Debug, Clone)]
pub enum HandleTarget {
    Invocation {
        id: String,
    },
    Service {
        name: String,
        handler: String,
        idempotency_key: String,
    },
    Object {
        name: String,
        key: String,
        handler: String,
        idempotency_key: String,
    },
    Workflow {
        name: String,
        id: String,
    },
}

impl HandleTarget {
    pub fn invocation(id: impl Into<String>) -> Self {
        Self::Invocation { id: id.into() }
    }

    pub fn service(
        name: impl Into<String>,
        handler: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self::Service {
            name: name.into(),
            handler: handler.into(),
            idempotency_key: idempotency_key.into(),
        }
    }

    pub fn object(
        name: impl Into<String>,
        key: impl Into<String>,
        handler: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self::Object {
            name: name.into(),
            key: key.into(),
            handler: handler.into(),
            idempotency_key: idempotency_key.into(),
        }
    }

    pub fn workflow(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self::Workflow {
            name: name.into(),
            id: id.into(),
        }
    }
}

impl Display for HandleTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HandleTarget::Invocation { id } => {
                write!(f, "restate/invocation/{id}")
            }
            HandleTarget::Service {
                name,
                handler,
                idempotency_key,
            } => {
                write!(f, "restate/invocation/{name}/{handler}/{idempotency_key}")
            }
            HandleTarget::Object {
                name,
                key,
                handler,
                idempotency_key,
            } => write!(
                f,
                "restate/invocation/{name}/{key}/{handler}/{idempotency_key}"
            ),
            HandleTarget::Workflow { name, id } => {
                write!(f, "restate/workflow/{name}/{id}")
            }
        }
    }
}

/// The mode of operation to use on the handle of the invocation or workflow.
#[derive(Debug, Clone, Copy)]
pub enum HandleOp {
    Attach,
    Output,
}

impl Display for HandleOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            HandleOp::Attach => write!(f, "attach"),
            HandleOp::Output => write!(f, "output"),
        }
    }
}

/// This struct encapsulates the parameters for operating on the handle of an invocation or workflow.
pub struct IngressHandle<'a, Res = ()> {
    inner: &'a IngressInternal,
    target: HandleTarget,
    res: PhantomData<Res>,
    opts: IngressHandleOptions,
}

#[derive(Default)]
pub(super) struct IngressHandleOptions {
    pub(super) timeout: Option<Duration>,
}

impl<'a, Res> IngressHandle<'a, Res> {
    pub(super) fn new(inner: &'a IngressInternal, target: HandleTarget) -> Self {
        Self {
            inner,
            target,
            res: PhantomData,
            opts: Default::default(),
        }
    }

    /// Set the timeout for the request.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.opts.timeout = Some(timeout);
        self
    }

    /// Attach to the invocation or workflow and wait for it to finish.
    pub async fn attach(self) -> Result<Res, IngressClientError>
    where
        Res: Deserialize + 'static,
    {
        self.inner
            .handle(self.target, HandleOp::Attach, self.opts)
            .await
    }

    /// Peek at the output of the invocation or workflow.
    pub async fn output(self) -> Result<Res, IngressClientError>
    where
        Res: Deserialize + 'static,
    {
        self.inner
            .handle(self.target, HandleOp::Output, self.opts)
            .await
    }
}
