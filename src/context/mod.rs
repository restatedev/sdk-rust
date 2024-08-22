//! Types exposing Restate functionalities to service handlers.

use crate::endpoint::{ContextInternal, InputMetadata};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

mod request;
mod run;

pub use request::{Request, RequestTarget};
pub use run::RunClosure;
pub type HeaderMap = http::HeaderMap<String>;

/// Service handler context.
pub struct Context<'ctx> {
    random_seed: u64,
    #[cfg(feature = "rand")]
    std_rng: rand::prelude::StdRng,
    headers: HeaderMap,
    inner: &'ctx ContextInternal,
}

impl<'ctx> Context<'ctx> {
    /// Get request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get request headers.
    pub fn headers_mut(&mut self) -> &HeaderMap {
        &mut self.headers
    }
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for Context<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            random_seed: value.1.random_seed,
            #[cfg(feature = "rand")]
            std_rng: rand::prelude::SeedableRng::seed_from_u64(value.1.random_seed),
            headers: value.1.headers,
            inner: value.0,
        }
    }
}

/// Object shared handler context.
pub struct SharedObjectContext<'ctx> {
    key: String,
    random_seed: u64,
    #[cfg(feature = "rand")]
    std_rng: rand::prelude::StdRng,
    headers: HeaderMap,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> SharedObjectContext<'ctx> {
    /// Get object key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get request headers.
    pub fn headers_mut(&mut self) -> &HeaderMap {
        &mut self.headers
    }
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for SharedObjectContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            random_seed: value.1.random_seed,
            #[cfg(feature = "rand")]
            std_rng: rand::prelude::SeedableRng::seed_from_u64(value.1.random_seed),
            headers: value.1.headers,
            inner: value.0,
        }
    }
}

/// Object handler context.
pub struct ObjectContext<'ctx> {
    key: String,
    random_seed: u64,
    #[cfg(feature = "rand")]
    std_rng: rand::prelude::StdRng,
    headers: HeaderMap,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> ObjectContext<'ctx> {
    /// Get object key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get request headers.
    pub fn headers_mut(&mut self) -> &HeaderMap {
        &mut self.headers
    }
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for ObjectContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            random_seed: value.1.random_seed,
            #[cfg(feature = "rand")]
            std_rng: rand::prelude::SeedableRng::seed_from_u64(value.1.random_seed),
            headers: value.1.headers,
            inner: value.0,
        }
    }
}

/// Workflow shared handler context.
pub struct SharedWorkflowContext<'ctx> {
    key: String,
    random_seed: u64,
    #[cfg(feature = "rand")]
    std_rng: rand::prelude::StdRng,
    headers: HeaderMap,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for SharedWorkflowContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            random_seed: value.1.random_seed,
            #[cfg(feature = "rand")]
            std_rng: rand::prelude::SeedableRng::seed_from_u64(value.1.random_seed),
            headers: value.1.headers,
            inner: value.0,
        }
    }
}

impl<'ctx> SharedWorkflowContext<'ctx> {
    /// Get workflow key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get request headers.
    pub fn headers_mut(&mut self) -> &HeaderMap {
        &mut self.headers
    }
}

/// Workflow handler context.
pub struct WorkflowContext<'ctx> {
    key: String,
    random_seed: u64,
    #[cfg(feature = "rand")]
    std_rng: rand::prelude::StdRng,
    headers: HeaderMap,
    pub(crate) inner: &'ctx ContextInternal,
}

impl<'ctx> From<(&'ctx ContextInternal, InputMetadata)> for WorkflowContext<'ctx> {
    fn from(value: (&'ctx ContextInternal, InputMetadata)) -> Self {
        Self {
            key: value.1.key,
            random_seed: value.1.random_seed,
            #[cfg(feature = "rand")]
            std_rng: rand::prelude::SeedableRng::seed_from_u64(value.1.random_seed),
            headers: value.1.headers,
            inner: value.0,
        }
    }
}

impl<'ctx> WorkflowContext<'ctx> {
    /// Get workflow key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get request headers.
    pub fn headers_mut(&mut self) -> &HeaderMap {
        &mut self.headers
    }
}

/// Trait exposing Restate timers functionalities.
pub trait ContextTimers<'ctx>: private::SealedContext<'ctx> {
    /// Sleep using Restate
    fn sleep(&self, duration: Duration) -> impl Future<Output = Result<(), TerminalError>> + 'ctx {
        private::SealedContext::inner_context(self).sleep(duration)
    }
}

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextTimers<'ctx> for CTX {}

/// Trait exposing Restate service to service communication functionalities.
pub trait ContextClient<'ctx>: private::SealedContext<'ctx> {
    /// Create a [`Request`].
    fn request<Req, Res>(
        &self,
        request_target: RequestTarget,
        req: Req,
    ) -> Request<'ctx, Req, Res> {
        Request::new(self.inner_context(), request_target, req)
    }

    /// Create a service client. The service client is generated by the [`restate_sdk_macros::service`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::service]
    /// trait MyService {
    ///   fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    ///     let client = ctx.service_client::<MyServiceClient>();
    ///
    ///     // Do request
    ///     let result = client.handle().call().await;
    ///
    ///     // Just send the request, don't wait the response
    ///     client.handle().send();
    ///
    ///     // Schedule the request to be executed later
    ///     client.handle().send_with_delay(Duration::from_secs(60));
    /// }
    ///
    /// ```
    fn service_client<C>(&self) -> C
    where
        C: IntoServiceClient<'ctx>,
    {
        C::create_client(self.inner_context())
    }

    /// Create an object client. The object client is generated by the [`restate_sdk_macros::object`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::object]
    /// trait MyObject {
    ///   fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    ///     let client = ctx.object_client::<MyObjectClient>("my-key");
    ///
    ///     // Do request
    ///     let result = client.handle().call().await;
    ///
    ///     // Just send the request, don't wait the response
    ///     client.handle().send();
    ///
    ///     // Schedule the request to be executed later
    ///     client.handle().send_with_delay(Duration::from_secs(60));
    /// }
    ///
    /// ```
    fn object_client<C>(&self, key: impl Into<String>) -> C
    where
        C: IntoObjectClient<'ctx>,
    {
        C::create_client(self.inner_context(), key.into())
    }

    /// Create an workflow client. The workflow client is generated by the [`restate_sdk_macros::workflow`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::workflow]
    /// trait MyWorkflow {
    ///   fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    ///     let client = ctx.workflow_client::<MyWorkflowClient>("my-key");
    ///
    ///     // Do request
    ///     let result = client.handle().call().await;
    ///
    ///     // Just send the request, don't wait the response
    ///     client.handle().send();
    ///
    ///     // Schedule the request to be executed later
    ///     client.handle().send_with_delay(Duration::from_secs(60));
    /// }
    ///
    /// ```
    fn workflow_client<C>(&self, key: impl Into<String>) -> C
    where
        C: IntoWorkflowClient<'ctx>,
    {
        C::create_client(self.inner_context(), key.into())
    }
}

/// Trait used by codegen to create the service client.
pub trait IntoServiceClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal) -> Self;
}

/// Trait used by codegen to use the object client.
pub trait IntoObjectClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal, key: String) -> Self;
}

/// Trait used by codegen to use the workflow client.
pub trait IntoWorkflowClient<'ctx>: Sized {
    fn create_client(ctx: &'ctx ContextInternal, key: String) -> Self;
}

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextClient<'ctx> for CTX {}

/// Trait exposing Restate awakeables functionalities.
///
/// Awakeables can be used to implement external asynchronous systems interactions,
/// for example you can send a Kafka record including the awakeable id returned by [`ContextAwakeables::awakeable`],
/// and then let another service consume from Kafka the responses of given external system interaction by using
/// [`ContextAwakeables::resolve_awakeable`] or [`ContextAwakeables::reject_awakeable`].
pub trait ContextAwakeables<'ctx>: private::SealedContext<'ctx> {
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

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextAwakeables<'ctx> for CTX {}

/// Trait exposing Restate functionalities to deal with non-deterministic operations.
pub trait ContextSideEffects<'ctx>: private::SealedContext<'ctx> {
    /// Run a non-deterministic operation and record its result.
    ///
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

    /// Return a random seed inherently predictable, based on the invocation id, which is not secret.
    ///
    /// This value is stable during the invocation lifecycle, thus across retries.
    fn random_seed(&self) -> u64 {
        private::SealedContext::random_seed(self)
    }

    /// Return a [`rand::Rng`] instance inherently predictable, seeded with [`ContextSideEffects::random_seed`].
    ///
    /// This instance is useful to generate identifiers, idempotency keys, and for uniform sampling from a set of options.
    /// If a cryptographically secure value is needed, please generate that externally using [`ContextSideEffects::run`].
    #[cfg(feature = "rand")]
    fn rand(&mut self) -> &mut rand::prelude::StdRng {
        private::SealedContext::rand(self)
    }

    /// Return a random [`uuid::Uuid`], generated using [`ContextSideEffects::rand`].
    #[cfg(all(feature = "rand", feature = "uuid"))]
    fn rand_uuid(&mut self) -> uuid::Uuid {
        let rand = private::SealedContext::rand(self);
        uuid::Uuid::from_u64_pair(rand::RngCore::next_u64(rand), rand::RngCore::next_u64(rand))
    }
}

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextSideEffects<'ctx> for CTX {}

/// Trait exposing Restate K/V read functionalities.
pub trait ContextReadState<'ctx>: private::SealedContext<'ctx> {
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

impl<'ctx, CTX: private::SealedContext<'ctx> + private::SealedCanReadState> ContextReadState<'ctx>
    for CTX
{
}

/// Trait exposing Restate K/V write functionalities.
pub trait ContextWriteState<'ctx>: private::SealedContext<'ctx> {
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

impl<'ctx, CTX: private::SealedContext<'ctx> + private::SealedCanWriteState> ContextWriteState<'ctx>
    for CTX
{
}

/// Trait exposing Restate promise functionalities.
///
/// A promise is a durable, distributed version of a Rust oneshot channel.
/// Restate keeps track of the promises across restarts/failures.
///
/// You can use this feature to implement interaction between different workflow handlers, e.g. to
/// send a signal from a shared handler to the workflow handler.
pub trait ContextPromises<'ctx>: private::SealedContext<'ctx> {
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

impl<'ctx, CTX: private::SealedContext<'ctx> + private::SealedCanUsePromises> ContextPromises<'ctx>
    for CTX
{
}

mod private {
    use super::*;

    pub trait SealedContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal;

        fn random_seed(&self) -> u64;

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng;
    }

    // Context capabilities
    pub trait SealedCanReadState {}
    pub trait SealedCanWriteState {}
    pub trait SealedCanUsePromises {}

    impl<'ctx> SealedContext<'ctx> for Context<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }

        fn random_seed(&self) -> u64 {
            self.random_seed
        }

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng {
            &mut self.std_rng
        }
    }

    impl<'ctx> SealedContext<'ctx> for SharedObjectContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }

        fn random_seed(&self) -> u64 {
            self.random_seed
        }

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng {
            &mut self.std_rng
        }
    }

    impl SealedCanReadState for SharedObjectContext<'_> {}

    impl<'ctx> SealedContext<'ctx> for ObjectContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }

        fn random_seed(&self) -> u64 {
            self.random_seed
        }

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng {
            &mut self.std_rng
        }
    }

    impl SealedCanReadState for ObjectContext<'_> {}
    impl SealedCanWriteState for ObjectContext<'_> {}

    impl<'ctx> SealedContext<'ctx> for SharedWorkflowContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }

        fn random_seed(&self) -> u64 {
            self.random_seed
        }

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng {
            &mut self.std_rng
        }
    }

    impl SealedCanReadState for SharedWorkflowContext<'_> {}
    impl SealedCanUsePromises for SharedWorkflowContext<'_> {}

    impl<'ctx> SealedContext<'ctx> for WorkflowContext<'ctx> {
        fn inner_context(&self) -> &'ctx ContextInternal {
            self.inner
        }

        fn random_seed(&self) -> u64 {
            self.random_seed
        }

        #[cfg(feature = "rand")]
        fn rand(&mut self) -> &mut rand::prelude::StdRng {
            &mut self.std_rng
        }
    }

    impl SealedCanReadState for WorkflowContext<'_> {}
    impl SealedCanWriteState for WorkflowContext<'_> {}
    impl SealedCanUsePromises for WorkflowContext<'_> {}
}
