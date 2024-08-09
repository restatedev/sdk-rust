use crate::endpoint::ContextInternal;
use std::future::Future;

pub trait Service {
    type Future: Future<Output = ()> + Send + 'static;

    fn handle(&self, req: ContextInternal) -> Self::Future;
}

pub trait Discoverable {
    fn discover() -> crate::discovery::Service;
}
