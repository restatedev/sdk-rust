use std::{
    fmt::{self, Display, Formatter},
    marker::PhantomData,
    time::Duration,
};

use serde::de::DeserializeOwned;

use super::internal::IngressInternal;
use crate::errors::TerminalError;

/// The invocation or workflow target to retrieve the result from.
#[derive(Debug, Clone)]
pub enum ResultTarget {
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

impl ResultTarget {
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

impl Display for ResultTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ResultTarget::Invocation { id } => {
                write!(f, "restate/invocation/{id}")
            }
            ResultTarget::Service {
                name,
                handler,
                idempotency_key,
            } => {
                write!(f, "restate/invocation/{name}/{handler}/{idempotency_key}")
            }
            ResultTarget::Object {
                name,
                key,
                handler,
                idempotency_key,
            } => write!(
                f,
                "restate/invocation/{name}/{key}/{handler}/{idempotency_key}"
            ),
            ResultTarget::Workflow { name, id } => {
                write!(f, "restate/workflow/{name}/{id}")
            }
        }
    }
}

/// The mode of operation to use when retrieving the result of an invocation or workflow.
#[derive(Debug, Clone, Copy)]
pub enum ResultOp {
    Attach,
    Output,
}

impl Display for ResultOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ResultOp::Attach => write!(f, "attach"),
            ResultOp::Output => write!(f, "output"),
        }
    }
}

/// This struct encapsulates the parameters for retrieving a result of an invocation or workflow.
pub struct IngressResult<'a, Res = ()> {
    inner: &'a IngressInternal,
    target: ResultTarget,
    res: PhantomData<Res>,
    opts: IngressResultOptions,
}

#[derive(Default)]
pub(super) struct IngressResultOptions {
    pub(super) timeout: Option<Duration>,
}

impl<'a, Res> IngressResult<'a, Res> {
    pub(super) fn new(inner: &'a IngressInternal, target: ResultTarget) -> Self {
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

    /// Attach to an invocation or workflow and wait for it to finish.
    pub async fn attach(self) -> Result<Result<Res, TerminalError>, reqwest::Error>
    where
        Res: DeserializeOwned + 'static,
    {
        self.inner
            .result(self.target, ResultOp::Attach, self.opts)
            .await
    }

    /// Peek at the output of an invocation or workflow.
    pub async fn output(self) -> Result<Result<Res, TerminalError>, reqwest::Error>
    where
        Res: DeserializeOwned + 'static,
    {
        self.inner
            .result(self.target, ResultOp::Output, self.opts)
            .await
    }
}
