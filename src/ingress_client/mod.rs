use reqwest::{header::HeaderMap, Url};

use self::{
    internal::IngressInternal,
    request::IngressRequest,
    result::{IngressResult, ResultTarget},
};
use crate::context::RequestTarget;

pub mod internal;
pub mod request;
pub mod result;

/// A client for invoking handlers via the ingress.
pub struct IngressClient {
    inner: IngressInternal,
}

impl IngressClient {
    /// Create a new [`IngressClient`].
    pub fn new(url: Url) -> Self {
        Self {
            inner: IngressInternal {
                client: reqwest::Client::new(),
                url,
                headers: Default::default(),
            },
        }
    }

    /// Create a new [`IngressClient`] with custom headers.
    pub fn new_with_headers(url: Url, headers: HeaderMap) -> Self {
        Self {
            inner: IngressInternal {
                client: reqwest::Client::new(),
                url,
                headers,
            },
        }
    }

    /// Create a new [`IngressRequest`].
    pub fn request<Req, Res>(&self, target: RequestTarget, req: Req) -> IngressRequest<Req, Res> {
        IngressRequest::new(&self.inner, target, req)
    }

    /// Create a new [`IngressResult`].
    pub fn result<Res>(&self, target: ResultTarget) -> IngressResult<Res> {
        IngressResult::new(&self.inner, target)
    }

    pub fn service_ingress<'a, I>(&'a self) -> I
    where
        I: IntoServiceIngress<'a>,
    {
        I::create_ingress(self)
    }

    pub fn object_ingress<'a, I>(&'a self, key: impl Into<String>) -> I
    where
        I: IntoObjectIngress<'a>,
    {
        I::create_ingress(self, key.into())
    }

    pub fn workflow_ingress<'a, I>(&'a self, id: impl Into<String>) -> I
    where
        I: IntoWorkflowIngress<'a>,
    {
        I::create_ingress(self, id.into())
    }

    pub fn invocation_result<'a, Res>(
        &'a self,
        invocation_id: impl Into<String>,
    ) -> IngressResult<'a, Res> {
        self.result(ResultTarget::invocation(invocation_id))
    }

    pub fn service_result<'a, R>(&'a self) -> R
    where
        R: IntoServiceResult<'a>,
    {
        R::create_result(self)
    }

    pub fn object_result<'a, R>(&'a self, key: impl Into<String>) -> R
    where
        R: IntoObjectResult<'a>,
    {
        R::create_result(self, key.into())
    }

    pub fn workflow_result<'a, R>(&'a self, id: impl Into<String>) -> R
    where
        R: IntoWorkflowResult<'a>,
    {
        R::create_result(self, id.into())
    }
}

/// Trait used by codegen to use the service ingress.
pub trait IntoServiceIngress<'a>: Sized {
    fn create_ingress(client: &'a IngressClient) -> Self;
}

/// Trait used by codegen to use the object ingress.
pub trait IntoObjectIngress<'a>: Sized {
    fn create_ingress(client: &'a IngressClient, key: String) -> Self;
}

/// Trait used by codegen to use the workflow ingress.
pub trait IntoWorkflowIngress<'a>: Sized {
    fn create_ingress(client: &'a IngressClient, id: String) -> Self;
}

/// Trait used by codegen to retrieve the service result.
pub trait IntoServiceResult<'a>: Sized {
    fn create_result(client: &'a IngressClient) -> Self;
}

/// Trait used by codegen to retrieve the object result.
pub trait IntoObjectResult<'a>: Sized {
    fn create_result(client: &'a IngressClient, key: String) -> Self;
}

/// Trait used by codegen to retrieve the workflow result.
pub trait IntoWorkflowResult<'a>: Sized {
    fn create_result(client: &'a IngressClient, id: String) -> Self;
}
