use crate::endpoint;
use futures::future::BoxFuture;
use std::future::Future;

/// Trait representing a Restate service.
///
/// This is used by codegen.
pub trait Service {
    type Future: Future<Output = Result<(), endpoint::Error>> + Send + 'static;

    /// Handle an incoming request.
    fn handle(&self, req: endpoint::ContextInternal) -> Self::Future;
}

/// Trait representing a discoverable Restate service.
///
/// This is used by codegen.
pub trait Discoverable {
    fn discover() -> crate::discovery::Service;
}

/// Used by codegen
#[doc(hidden)]
pub type ServiceBoxFuture = BoxFuture<'static, Result<(), endpoint::Error>>;
