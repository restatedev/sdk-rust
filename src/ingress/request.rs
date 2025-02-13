use std::{marker::PhantomData, time::Duration};

use http::HeaderValue;
use serde::{de::DeserializeOwned, Serialize};

use super::internal::IngressInternal;
use crate::{context::RequestTarget, errors::TerminalError};

/// A send response.
#[derive(Debug, Clone)]
pub struct SendResponse {
    pub invocation_id: String,
    pub status: SendStatus,
    pub attachable: bool,
}

/// The status of the send.
#[derive(Debug, Clone, Copy)]
pub enum SendStatus {
    Accepted,
    PreviouslyAccepted,
}

/// This struct encapsulates the parameters for a request to an ingress.
pub struct IngressRequest<'a, Req, Res = ()> {
    inner: &'a IngressInternal,
    target: RequestTarget,
    req: Req,
    res: PhantomData<Res>,
    opts: IngressRequestOptions,
}

#[derive(Default, Clone)]
pub(super) struct IngressRequestOptions {
    pub(super) idempotency_key: Option<HeaderValue>,
    pub(super) timeout: Option<Duration>,
}

impl<'a, Req, Res> IngressRequest<'a, Req, Res> {
    pub(super) fn new(inner: &'a IngressInternal, target: RequestTarget, req: Req) -> Self {
        Self {
            inner,
            target,
            req,
            res: PhantomData,
            opts: Default::default(),
        }
    }

    /// Set the idempotency key for the request.
    pub fn idempotency_key(mut self, value: HeaderValue) -> Self {
        self.opts.idempotency_key = Some(value);
        self
    }

    /// Set the timeout for the request.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.opts.timeout = Some(value);
        self
    }

    /// Call a service via the ingress. This returns a future encapsulating the response.
    pub async fn call(self) -> Result<Result<Res, TerminalError>, reqwest::Error>
    where
        Req: Serialize + 'static,
        Res: DeserializeOwned + 'static,
    {
        self.inner.call(self.target, self.req, self.opts).await
    }

    /// Send the request to the ingress, without waiting for the response.
    pub async fn send(self) -> Result<Result<SendResponse, TerminalError>, reqwest::Error>
    where
        Req: Serialize + 'static,
    {
        self.inner
            .send(self.target, self.req, self.opts, None)
            .await
    }

    /// Schedule the request to the ingress, without waiting for the response.
    pub async fn send_with_delay(
        self,
        duration: Duration,
    ) -> Result<Result<SendResponse, TerminalError>, reqwest::Error>
    where
        Req: Serialize + 'static,
    {
        self.inner
            .send(self.target, self.req, self.opts, Some(duration))
            .await
    }
}
