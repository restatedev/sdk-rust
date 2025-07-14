//! Hyper integration.

use crate::endpoint;
use crate::endpoint::Endpoint;

use http::{Request, Response};
use hyper::body::Incoming;
use hyper::service::Service;
use std::future::{ready, Ready};

/// Wraps [`Endpoint`] to implement hyper [`Service`].
#[derive(Clone)]
pub struct HyperEndpoint(Endpoint);

impl HyperEndpoint {
    pub fn new(endpoint: Endpoint) -> Self {
        Self(endpoint)
    }
}

impl Service<Request<Incoming>> for HyperEndpoint {
    type Response = Response<endpoint::ResponseBody>;
    type Error = endpoint::Error;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        ready(self.0.handle(req))
    }
}
