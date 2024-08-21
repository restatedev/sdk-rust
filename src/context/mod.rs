use crate::endpoint::{ContextInternal, InputMetadata};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

mod request;
mod run;

pub use request::{Request, RequestTarget};
pub use run::RunClosure;

pub struct Context<'a> {
    inner: &'a ContextInternal,
}

impl<'a> From<(&'a ContextInternal, InputMetadata)> for Context<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self { inner: value.0 }
    }
}

pub struct SharedObjectContext<'a> {
    key: String,
    pub(crate) inner: &'a ContextInternal,
}

impl<'a> SharedObjectContext<'a> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl<'a> From<(&'a ContextInternal, InputMetadata)> for SharedObjectContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

pub struct ObjectContext<'a> {
    key: String,
    pub(crate) inner: &'a ContextInternal,
}

impl<'a> ObjectContext<'a> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl<'a> From<(&'a ContextInternal, InputMetadata)> for ObjectContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

pub struct SharedWorkflowContext<'a> {
    key: String,
    pub(crate) inner: &'a ContextInternal,
}

impl<'a> From<(&'a ContextInternal, InputMetadata)> for SharedWorkflowContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

impl<'a> SharedWorkflowContext<'a> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

pub struct WorkflowContext<'a> {
    key: String,
    pub(crate) inner: &'a ContextInternal,
}

impl<'a> From<(&'a ContextInternal, InputMetadata)> for WorkflowContext<'a> {
    fn from(value: (&'a ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

impl<'a> WorkflowContext<'a> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

pub trait ContextTimers<'a>: private::SealedGetInnerContext<'a> {
    /// Sleep using Restate
    fn sleep(&self, duration: Duration) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        private::SealedGetInnerContext::inner_context(self).sleep(duration)
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a>> ContextTimers<'a> for CTX {}

pub trait ContextClient<'a>: private::SealedGetInnerContext<'a> {
    fn request<Req: Serialize + 'static>(
        &self,
        request_target: RequestTarget,
        req: Req,
    ) -> Request<'a, Req> {
        Request::new(self.inner_context(), request_target, req)
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a>> ContextClient<'a> for CTX {}

pub trait ContextAwakeables<'a>: private::SealedGetInnerContext<'a> {
    /// Create an awakeable
    fn awakeable<T: Deserialize + 'static>(
        &self,
    ) -> (
        String,
        impl Future<Output = Result<T, TerminalError>> + Send + Sync + 'a,
    ) {
        self.inner_context().awakeable()
    }

    /// Resolve an awakeable
    fn resolve_awakeable<T: Serialize + 'static>(&self, key: &str, t: T) {
        self.inner_context().resolve_awakeable(key, t)
    }

    /// Resolve an awakeable
    fn reject_awakeable(&self, key: &str, failure: TerminalError) {
        self.inner_context().reject_awakeable(key, failure)
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a>> ContextAwakeables<'a> for CTX {}

pub trait ContextSideEffects<'a>: private::SealedGetInnerContext<'a> {
    /// Run a non-deterministic operation
    fn run<R, F, T>(
        &self,
        name: &'a str,
        run_closure: R,
    ) -> impl Future<Output = Result<T, TerminalError>> + 'a
    where
        R: RunClosure<Fut = F, Output = T> + Send + Sync + 'a,
        T: Serialize + Deserialize,
        F: Future<Output = HandlerResult<T>> + Send + Sync + 'a,
    {
        self.inner_context().run(name, run_closure)
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a>> ContextSideEffects<'a> for CTX {}

pub trait ContextReadState<'a>: private::SealedGetInnerContext<'a> {
    /// Get state
    fn get<T: Deserialize + 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.inner_context().get(key)
    }

    /// Get state keys
    fn get_keys(&self) -> impl Future<Output = Result<Vec<String>, TerminalError>> + 'a {
        self.inner_context().get_keys()
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a> + private::SealedCanReadState> ContextReadState<'a>
    for CTX
{
}

pub trait ContextWriteState<'a>: private::SealedGetInnerContext<'a> {
    /// Set state
    fn set<T: Serialize + 'static>(&self, key: &str, t: T) {
        self.inner_context().set(key, t)
    }

    /// Clear state
    fn clear(&self, key: &str) {
        self.inner_context().clear(key)
    }

    /// Clear all state
    fn clear_all(&self) {
        self.inner_context().clear_all()
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a> + private::SealedCanWriteState>
    ContextWriteState<'a> for CTX
{
}

pub trait ContextPromises<'a>: private::SealedGetInnerContext<'a> {
    /// Create a promise
    fn promise<T: Deserialize + 'static>(
        &'a self,
        key: &str,
    ) -> impl Future<Output = Result<T, TerminalError>> + 'a {
        self.inner_context().promise(key)
    }

    /// Peek a promise
    fn peek_promise<T: Deserialize + 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'a {
        self.inner_context().peek_promise(key)
    }

    /// Resolve a promise
    fn resolve_promise<T: Serialize + 'static>(&self, key: &str, t: T) {
        self.inner_context().resolve_promise(key, t)
    }

    /// Resolve a promise
    fn reject_promise(&self, key: &str, failure: TerminalError) {
        self.inner_context().reject_promise(key, failure)
    }
}

impl<'a, CTX: private::SealedGetInnerContext<'a> + private::SealedCanUsePromises>
    ContextPromises<'a> for CTX
{
}

mod private {
    use super::*;

    pub trait SealedGetInnerContext<'a> {
        fn inner_context(&self) -> &'a ContextInternal;
    }

    // Context capabilities
    pub trait SealedCanReadState {}
    pub trait SealedCanWriteState {}
    pub trait SealedCanUsePromises {}

    impl<'a> SealedGetInnerContext<'a> for Context<'a> {
        fn inner_context(&self) -> &'a ContextInternal {
            self.inner
        }
    }

    impl<'a> SealedGetInnerContext<'a> for SharedObjectContext<'a> {
        fn inner_context(&self) -> &'a ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for SharedObjectContext<'_> {}

    impl<'a> SealedGetInnerContext<'a> for ObjectContext<'a> {
        fn inner_context(&self) -> &'a ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for ObjectContext<'_> {}
    impl SealedCanWriteState for ObjectContext<'_> {}

    impl<'a> SealedGetInnerContext<'a> for SharedWorkflowContext<'a> {
        fn inner_context(&self) -> &'a ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for SharedWorkflowContext<'_> {}
    impl SealedCanUsePromises for SharedWorkflowContext<'_> {}

    impl<'a> SealedGetInnerContext<'a> for WorkflowContext<'a> {
        fn inner_context(&self) -> &'a ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for WorkflowContext<'_> {}
    impl SealedCanWriteState for WorkflowContext<'_> {}
    impl SealedCanUsePromises for WorkflowContext<'_> {}
}
