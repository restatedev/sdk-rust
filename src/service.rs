use crate::endpoint;
use futures::future::BoxFuture;
use std::future::Future;

pub trait Service {
    type Future: Future<Output = Result<(), endpoint::Error>> + Send + 'static;

    fn handle(&self, req: endpoint::ContextInternal) -> Self::Future;
}

pub trait Discoverable {
    fn discover() -> crate::discovery::Service;
}

/// Used by codegen
#[doc(hidden)]
pub type ServiceBoxFuture = BoxFuture<'static, Result<(), endpoint::Error>>;
