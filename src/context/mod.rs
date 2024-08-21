use crate::endpoint::{ContextInternal, InputMetadata};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

mod request;
mod run;

pub use request::{Request, RequestTarget};
pub use run::RunClosure;

pub struct Context<'ctx> {
    inner: &'ctx ContextInternal,
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for Context<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self { inner: value.0 }
    }
}

pub struct SharedObjectContext<'ctx> {
    key: String,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> SharedObjectContext<'ctx> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for SharedObjectContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

pub struct ObjectContext<'ctx> {
    key: String,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> ObjectContext<'ctx> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for ObjectContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

pub struct SharedWorkflowContext<'ctx> {
    key: String,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for SharedWorkflowContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

impl<'ctx> SharedWorkflowContext<'ctx> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

pub struct WorkflowContext<'ctx> {
    key: String,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for WorkflowContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            inner: value.0,
        }
    }
}

impl<'ctx> WorkflowContext<'ctx> {
    pub fn key(&self) -> &str {
        &self.key
    }
}

pub trait ContextTimers<'ctx>: private::SealedGetInnerContext<'ctx> {
    /// Sleep using Restate
    fn sleep(&self, duration: Duration) -> impl Future<Output = Result<(), TerminalError>> + 'ctx {
        private::SealedGetInnerContext::inner_context(self).sleep(duration)
    }
}

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx>> ContextTimers<'ctx> for CTX {}

pub trait ContextClient<'ctx>: private::SealedGetInnerContext<'ctx> {
    fn request<Req, Res>(
        &self,
        request_target: RequestTarget,
        req: Req,
    ) -> Request<'ctx, Req, Res> {
        Request::new(self.inner_context(), request_target, req)
    }

    fn service_client<C>(&self) -> C
    where
        C: IntoServiceClient<'ctx>,
    {
        C::create_client(self.inner_context())
    }

    fn object_client<C>(&self, key: impl Into<String>) -> C
    where
        C: IntoObjectClient<'ctx>,
    {
        C::create_client(self.inner_context(), key.into())
    }

    fn workflow_client<C>(&self, key: impl Into<String>) -> C
    where
        C: IntoWorkflowClient<'ctx>,
    {
        C::create_client(self.inner_context(), key.into())
    }
}

pub trait IntoServiceClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal) -> Self;
}

pub trait IntoObjectClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal, key: String) -> Self;
}

pub trait IntoWorkflowClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal, key: String) -> Self;
}

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx>> ContextClient<'ctx> for CTX {}

pub trait ContextAwakeables<'ctx>: private::SealedGetInnerContext<'ctx> {
    /// Create an awakeable
    fn awakeable<T: Deserialize + 'static>(
        &self,
    ) -> (
        String,
        impl Future<Output = Result<T, TerminalError>> + Send + Sync + 'ctx,
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

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx>> ContextAwakeables<'ctx> for CTX {}

pub trait ContextSideEffects<'ctx>: private::SealedGetInnerContext<'ctx> {
    /// Run a non-deterministic operation
    fn run<R, F, T>(
        &self,
        name: &'ctx str,
        run_closure: R,
    ) -> impl Future<Output = Result<T, TerminalError>> + 'ctx
    where
        R: RunClosure<Fut = F, Output = T> + Send + Sync + 'ctx,
        T: Serialize + Deserialize,
        F: Future<Output = HandlerResult<T>> + Send + Sync + 'ctx,
    {
        self.inner_context().run(name, run_closure)
    }
}

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx>> ContextSideEffects<'ctx> for CTX {}

pub trait ContextReadState<'ctx>: private::SealedGetInnerContext<'ctx> {
    /// Get state
    fn get<T: Deserialize + 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'ctx {
        self.inner_context().get(key)
    }

    /// Get state keys
    fn get_keys(&self) -> impl Future<Output = Result<Vec<String>, TerminalError>> + 'ctx {
        self.inner_context().get_keys()
    }
}

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx> + private::SealedCanReadState>
    ContextReadState<'ctx> for CTX
{
}

pub trait ContextWriteState<'ctx>: private::SealedGetInnerContext<'ctx> {
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

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx> + private::SealedCanWriteState>
    ContextWriteState<'ctx> for CTX
{
}

pub trait ContextPromises<'ctx>: private::SealedGetInnerContext<'ctx> {
    /// Create a promise
    fn promise<T: Deserialize + 'static>(
        &'ctx self,
        key: &str,
    ) -> impl Future<Output = Result<T, TerminalError>> + 'ctx {
        self.inner_context().promise(key)
    }

    /// Peek a promise
    fn peek_promise<T: Deserialize + 'static>(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<T>, TerminalError>> + 'ctx {
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

impl<'ctx, CTX: private::SealedGetInnerContext<'ctx> + private::SealedCanUsePromises>
    ContextPromises<'ctx> for CTX
{
}

mod private {
    use super::*;

    pub trait SealedGetInnerContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal;
    }

    // Context capabilities
    pub trait SealedCanReadState {}
    pub trait SealedCanWriteState {}
    pub trait SealedCanUsePromises {}

    impl<'ctx> SealedGetInnerContext<'ctx> for Context<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }
    }

    impl<'ctx> SealedGetInnerContext<'ctx> for SharedObjectContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for SharedObjectContext<'_> {}

    impl<'ctx> SealedGetInnerContext<'ctx> for ObjectContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for ObjectContext<'_> {}
    impl SealedCanWriteState for ObjectContext<'_> {}

    impl<'ctx> SealedGetInnerContext<'ctx> for SharedWorkflowContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for SharedWorkflowContext<'_> {}
    impl SealedCanUsePromises for SharedWorkflowContext<'_> {}

    impl<'ctx> SealedGetInnerContext<'ctx> for WorkflowContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }
    }

    impl SealedCanReadState for WorkflowContext<'_> {}
    impl SealedCanWriteState for WorkflowContext<'_> {}
    impl SealedCanUsePromises for WorkflowContext<'_> {}
}
