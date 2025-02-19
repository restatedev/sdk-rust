use reqwest::{header::HeaderMap, Url};

use self::{
    handle::{HandleTarget, IngressHandle},
    internal::IngressInternal,
    request::IngressRequest,
};
use crate::context::RequestTarget;

pub mod handle;
pub mod internal;
pub mod request;

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

    /// Create a new [`IngressHandle`].
    pub fn handle<Res>(&self, target: HandleTarget) -> IngressHandle<Res> {
        IngressHandle::new(&self.inner, target)
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

    pub fn invocation_handle<'a, Res>(
        &'a self,
        invocation_id: impl Into<String>,
    ) -> IngressHandle<'a, Res> {
        self.handle(HandleTarget::invocation(invocation_id))
    }

    pub fn service_handle<'a, H>(&'a self) -> H
    where
        H: IntoServiceHandle<'a>,
    {
        H::create_handle(self)
    }

    pub fn object_handle<'a, H>(&'a self, key: impl Into<String>) -> H
    where
        H: IntoObjectHandle<'a>,
    {
        H::create_handle(self, key.into())
    }

    pub fn workflow_handle<'a, H>(&'a self, id: impl Into<String>) -> H
    where
        H: IntoWorkflowHandle<'a>,
    {
        H::create_handle(self, id.into())
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

/// Trait used by codegen to use the service handle.
pub trait IntoServiceHandle<'a>: Sized {
    fn create_handle(client: &'a IngressClient) -> Self;
}

/// Trait used by codegen to use the object handle.
pub trait IntoObjectHandle<'a>: Sized {
    fn create_handle(client: &'a IngressClient, key: String) -> Self;
}

/// Trait used by codegen to use the workflow handle.
pub trait IntoWorkflowHandle<'a>: Sized {
    fn create_handle(client: &'a IngressClient, id: String) -> Self;
}
