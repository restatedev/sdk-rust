//! Types exposing Restate functionalities to service handlers.

use crate::endpoint::{ContextInternal, InputMetadata};
use crate::errors::{HandlerResult, TerminalError};
use crate::serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

#[doc(hidden)]
pub mod macro_support;
mod request;
mod run;
mod select;

pub use request::{CallFuture, InvocationHandle, Request, RequestTarget};
pub use run::{RunClosure, RunFuture, RunRetryPolicy};

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

///
/// # Scheduling & Timers
/// The Restate SDK includes durable timers.
/// You can use these to let handlers sleep for a specified time, or to schedule a handler to be called at a later time.
/// These timers are resilient to failures and restarts.
/// Restate stores and keeps track of the timers and triggers them on time, even across failures and restarts.
///
/// ## Scheduling Async Tasks
///
/// To schedule a handler to be called at a later time, have a look at the documentation on [delayed calls](Request::send_after).
///
/// ## Durable sleep
/// To sleep in a Restate application for ten seconds, do the following:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # use std::convert::Infallible;
/// # use std::time::Duration;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// ctx.sleep(Duration::from_secs(10)).await?;
/// #    Ok(())
/// # }
/// ```
///
/// **Cost savings on FaaS**:
/// Restate suspends the handler while it is sleeping, to free up resources.
/// This is beneficial for AWS Lambda deployments, since you don't pay for the time the handler is sleeping.
///
/// **Sleeping in Virtual Objects**:
/// Virtual Objects only process a single invocation at a time, so the Virtual Object will be blocked while sleeping.
///
/// <details>
/// <summary>Clock synchronization Restate Server vs. SDK</summary>
///
/// The Restate SDK calculates the wake-up time based on the delay you specify.
/// The Restate Server then uses this calculated time to wake up the handler.
/// If the Restate Server and the SDK have different system clocks, the sleep duration might not be accurate.
/// So make sure that the system clock of the Restate Server and the SDK have the same timezone and are synchronized.
/// </details>
pub trait ContextTimers<'ctx>: private::SealedContext<'ctx> {
    /// Sleep using Restate
    fn sleep(
        &self,
        duration: Duration,
    ) -> impl DurableFuture<Output = Result<(), TerminalError>> + 'ctx {
        private::SealedContext::inner_context(self).sleep(duration)
    }
}

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextTimers<'ctx> for CTX {}

/// # Service Communication
///
/// A handler can call another handler and wait for the response (request-response), or it can send a message without waiting for the response.
///
/// ## Request-response calls
///
/// Request-response calls are requests where the client waits for the response.
///
/// You can do request-response calls to Services, Virtual Objects, and Workflows, in the following way:
///
/// ```rust,no_run
/// # #[path = "../../examples/services/mod.rs"]
/// # mod services;
/// # use services::my_virtual_object::MyVirtualObjectClient;
/// # use services::my_service::MyServiceClient;
/// # use services::my_workflow::MyWorkflowClient;
/// # use restate_sdk::prelude::*;
/// #
/// # async fn greet(ctx: Context<'_>, greeting: String) -> Result<(), HandlerError> {
///     // To a Service:
///     let service_response = ctx
///         .service_client::<MyServiceClient>()
///         .my_handler(String::from("Hi!"))
///         .call()
///         .await?;
///
///     // To a Virtual Object:
///     let object_response = ctx
///         .object_client::<MyVirtualObjectClient>("Mary")
///         .my_handler(String::from("Hi!"))
///         .call()
///         .await?;
///
///     // To a Workflow:
///     let workflow_result = ctx
///         .workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .run(String::from("Hi!"))
///         .call()
///         .await?;
///     ctx.workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .interact_with_workflow()
///         .call()
///         .await?;
///  #    Ok(())
///  # }
/// ```
///
/// 1. **Create a service client**.
///     - For Virtual Objects, you also need to supply the key of the Virtual Object you want to call, here `"Mary"`.
///     - For Workflows, you need to supply a workflow ID that is unique per workflow execution.
/// 2. **Specify the handler** you want to call and supply the request.
/// 3. **Await** the call to retrieve the response.
///
/// **No need for manual retry logic**:
/// Restate proxies all the calls and logs them in the journal.
/// In case of failures, Restate takes care of retries, so you don't need to implement this yourself here.
///
/// ## Sending messages
///
/// Handlers can send messages (a.k.a. one-way calls, or fire-and-forget calls), as follows:
///
/// ```rust,no_run
/// # #[path = "../../examples/services/mod.rs"]
/// # mod services;
/// # use services::my_virtual_object::MyVirtualObjectClient;
/// # use services::my_service::MyServiceClient;
/// # use services::my_workflow::MyWorkflowClient;
/// # use restate_sdk::prelude::*;
/// #
/// # async fn greet(ctx: Context<'_>, greeting: String) -> Result<(), HandlerError> {
///     // To a Service:
///     ctx.service_client::<MyServiceClient>()
///         .my_handler(String::from("Hi!"))
///         .send();
///
///     // To a Virtual Object:
///     ctx.object_client::<MyVirtualObjectClient>("Mary")
///         .my_handler(String::from("Hi!"))
///         .send();
///
///     // To a Workflow:
///     ctx.workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .run(String::from("Hi!"))
///         .send();
///     ctx.workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .interact_with_workflow()
///         .send();
///  #    Ok(())
///  # }
/// ```
///
/// **No need for message queues**:
/// Without Restate, you would usually put a message queue in between the two services, to guarantee the message delivery.
/// Restate eliminates the need for a message queue because Restate durably logs the request and makes sure it gets executed.
///
/// ## Delayed calls
///
/// A delayed call is a one-way call that gets executed after a specified delay.
///
/// To schedule a delayed call, send a message with a delay parameter, as follows:
///
/// ```rust,no_run
/// # #[path = "../../examples/services/mod.rs"]
/// # mod services;
/// # use services::my_virtual_object::MyVirtualObjectClient;
/// # use services::my_service::MyServiceClient;
/// # use services::my_workflow::MyWorkflowClient;
/// # use restate_sdk::prelude::*;
/// # use std::time::Duration;
/// #
/// # async fn greet(ctx: Context<'_>, greeting: String) -> Result<(), HandlerError> {
///     // To a Service:
///     ctx.service_client::<MyServiceClient>()
///         .my_handler(String::from("Hi!"))
///         .send_after(Duration::from_millis(5000));
///
///     // To a Virtual Object:
///     ctx.object_client::<MyVirtualObjectClient>("Mary")
///         .my_handler(String::from("Hi!"))
///         .send_after(Duration::from_millis(5000));
///
///     // To a Workflow:
///     ctx.workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .run(String::from("Hi!"))
///         .send_after(Duration::from_millis(5000));
///     ctx.workflow_client::<MyWorkflowClient>("my-workflow-id")
///         .interact_with_workflow()
///         .send_after(Duration::from_millis(5000));
///  #    Ok(())
///  # }
/// ```
///
/// You can also use this functionality to schedule async tasks.
/// Restate will make sure the task gets executed at the desired time.
///
/// ### Ordering guarantees in Virtual Objects
/// Invocations to a Virtual Object are executed serially.
/// Invocations will execute in the same order in which they arrive at Restate.
/// For example, assume a handler calls the same Virtual Object twice:
///
/// ```rust,no_run
/// # #[path = "../../examples/services/my_virtual_object.rs"]
/// # mod my_virtual_object;
/// # use my_virtual_object::MyVirtualObjectClient;
/// # use restate_sdk::prelude::*;
/// # async fn greet(ctx: Context<'_>, greeting: String) -> Result<(), HandlerError> {
///     ctx.object_client::<MyVirtualObjectClient>("Mary")
///         .my_handler(String::from("I'm call A!"))
///         .send();
///     ctx.object_client::<MyVirtualObjectClient>("Mary")
///         .my_handler(String::from("I'm call B!"))
///         .send();
/// #    Ok(())
/// }
/// ```
///
/// It is guaranteed that call A will execute before call B.
/// It is not guaranteed though that call B will be executed immediately after call A, as invocations coming from other handlers/sources, could interleave these two calls.
///
/// ### Deadlocks with Virtual Objects
/// Request-response calls to Virtual Objects can lead to deadlocks, in which the Virtual Object remains locked and can't process any more requests.
/// Some example cases:
/// - Cross deadlock between Virtual Object A and B: A calls B, and B calls A, both using same keys.
/// - Cyclical deadlock: A calls B, and B calls C, and C calls A again.
///
/// In this situation, you can use the CLI to unblock the Virtual Object manually by [cancelling invocations](https://docs.restate.dev/operate/invocation#cancelling-invocations).
///
pub trait ContextClient<'ctx>: private::SealedContext<'ctx> {
    /// Create a [`Request`].
    fn request<Req, Res>(
        &self,
        request_target: RequestTarget,
        req: Req,
    ) -> Request<'ctx, Req, Res> {
        Request::new(self.inner_context(), request_target, req)
    }

    /// Create an [`InvocationHandle`] from an invocation id.
    fn invocation_handle(&self, invocation_id: String) -> impl InvocationHandle + 'ctx {
        self.inner_context().invocation_handle(invocation_id)
    }

    /// Create a service client. The service client is generated by the [`restate_sdk_macros::service`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust,no_run
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::service]
    /// trait MyService {
    ///   async fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    /// let client = ctx.service_client::<MyServiceClient>();
    ///
    /// // Do request
    /// let result = client.handle().call().await;
    ///
    /// // Just send the request, don't wait the response
    /// client.handle().send();
    ///
    /// // Schedule the request to be executed later
    /// client.handle().send_after(Duration::from_secs(60));
    /// # }
    /// ```
    fn service_client<C>(&self) -> C
    where
        C: IntoServiceClient<'ctx>,
    {
        C::create_client(self.inner_context())
    }

    /// Create an object client. The object client is generated by the [`restate_sdk_macros::object`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust,no_run
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::object]
    /// trait MyObject {
    ///   async fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    /// let client = ctx.object_client::<MyObjectClient>("my-key");
    ///
    /// // Do request
    /// let result = client.handle().call().await;
    ///
    /// // Just send the request, don't wait the response
    /// client.handle().send();
    ///
    /// // Schedule the request to be executed later
    /// client.handle().send_after(Duration::from_secs(60));
    /// # }
    /// ```
    fn object_client<C>(&self, key: impl Into<String>) -> C
    where
        C: IntoObjectClient<'ctx>,
    {
        C::create_client(self.inner_context(), key.into())
    }

    /// Create an workflow client. The workflow client is generated by the [`restate_sdk_macros::workflow`] macro with the same name of the trait suffixed with `Client`.
    ///
    /// ```rust,no_run
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::workflow]
    /// trait MyWorkflow {
    ///   async fn handle() -> HandlerResult<()>;
    /// }
    ///
    /// # async fn handler(ctx: Context<'_>) {
    /// let client = ctx.workflow_client::<MyWorkflowClient>("my-key");
    ///
    /// // Do request
    /// let result = client.handle().call().await;
    ///
    /// // Just send the request, don't wait the response
    /// client.handle().send();
    ///
    /// // Schedule the request to be executed later
    /// client.handle().send_after(Duration::from_secs(60));
    /// # }
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

/// # Awakeables
///
/// Awakeables pause an invocation while waiting for another process to complete a task.
/// You can use this pattern to let a handler execute a task somewhere else and retrieve the result.
/// This pattern is also known as the callback (task token) pattern.
///
/// ## Creating awakeables
///
/// 1. **Create an awakeable**. This contains a String identifier and a Promise.
/// 2. **Trigger a task/process** and attach the awakeable ID (e.g., over Kafka, via HTTP, ...).
///    For example, send an HTTP request to a service that executes the task, and attach the ID to the payload.
///    You use `ctx.run` to avoid re-triggering the task on retries.
/// 3. **Wait** until the other process has executed the task.
///    The handler **receives the payload and resumes**.
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// #
/// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
/// /// 1. Create an awakeable
/// let (id, promise) = ctx.awakeable::<String>();
///
/// /// 2. Trigger a task
/// ctx.run(|| trigger_task_and_deliver_id(id.clone())).await?;
///
/// /// 3. Wait for the promise to be resolved
/// let payload = promise.await?;
/// # Ok(())
/// # }
/// # async fn trigger_task_and_deliver_id(awakeable_id: String) -> Result<(), HandlerError>{
/// #    Ok(())
/// # }
/// ```
///
///
/// ## Completing awakeables
///
/// The external process completes the awakeable by either resolving it with an optional payload or by rejecting it
/// with its ID and a reason for the failure. This throws [a terminal error][crate::errors] in the waiting handler.
///
/// - Resolving over HTTP with its ID and an optional payload:
///
/// ```text
/// curl localhost:8080/restate/awakeables/prom_1PePOqp/resolve
///     -H 'content-type: application/json'
///     -d '{"hello": "world"}'
/// ```
///
/// - Rejecting over HTTP with its ID and a reason:
///
/// ```text
/// curl localhost:8080/restate/awakeables/prom_1PePOqp/reject
///     -H 'content-type: text/plain' \
///     -d 'Very bad error!'
/// ```
///
/// - Resolving via the SDK with its ID and an optional payload, or rejecting with its ID and a reason:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// #
/// # async fn handle(ctx: Context<'_>, id: String) -> Result<(), HandlerError> {
/// // Resolve the awakeable
/// ctx.resolve_awakeable(&id, "hello".to_string());
///
/// // Or reject the awakeable
/// ctx.reject_awakeable(&id, TerminalError::new("my error reason"));
/// # Ok(())
/// # }
/// ```
///
/// For more info about serialization of the payload, see [crate::serde]
///
/// When running on Function-as-a-Service platforms, such as AWS Lambda, Restate suspends the handler while waiting for the awakeable to be completed.
/// Since you only pay for the time that the handler is actually running, you don't pay while waiting for the external process to return.
///
/// **Be aware**: Virtual Objects only process a single invocation at a time, so the Virtual Object will be blocked while waiting on the awakeable to be resolved.
pub trait ContextAwakeables<'ctx>: private::SealedContext<'ctx> {
    /// Create an awakeable
    fn awakeable<T: Deserialize + 'static>(
        &self,
    ) -> (
        String,
        impl DurableFuture<Output = Result<T, TerminalError>> + Send + 'ctx,
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

/// # Journaling Results
///
/// Restate uses an execution log for replay after failures and suspensions.
/// This means that non-deterministic results (e.g. database responses, UUID generation) need to be stored in the execution log.
/// The SDK offers some functionalities to help you with this:
/// 1. **[Journaled actions][crate::context::ContextSideEffects::run]**: Run any block of code and store the result in Restate. Restate replays the result instead of re-executing the block on retries.
/// 2. **[UUID generator][crate::context::ContextSideEffects::rand_uuid]**: Built-in helpers for generating stable UUIDs. Restate seeds the random number generator with the invocation ID, so it always returns the same value on retries.
/// 3. **[Random generator][crate::context::ContextSideEffects::rand]**: Built-in helpers for generating randoms. Restate seeds the random number generator with the invocation ID, so it always returns the same value on retries.
///
pub trait ContextSideEffects<'ctx>: private::SealedContext<'ctx> {
    /// ## Journaled actions
    /// You can store the result of a (non-deterministic) operation in the Restate execution log (e.g. database requests, HTTP calls, etc).
    /// Restate replays the result instead of re-executing the operation on retries.
    ///
    /// Here is an example of a database request for which the string response is stored in Restate:
    /// ```rust,no_run
    /// # use restate_sdk::prelude::*;
    /// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
    /// let response = ctx.run(|| do_db_request()).await?;
    /// # Ok(())
    /// # }
    /// # async fn do_db_request() -> Result<String, HandlerError>{
    /// # Ok("Hello".to_string())
    /// # }
    /// ```
    ///
    /// You cannot use the Restate context within `ctx.run`.
    /// This includes actions such as getting state, calling another service, and nesting other journaled actions.
    ///
    /// For more info about serialization of the return values, see [crate::serde].
    ///
    /// You can configure the retry policy for the `ctx.run` block:
    /// ```rust,no_run
    /// # use std::time::Duration;
    /// # use restate_sdk::prelude::*;
    /// # async fn handle(ctx: Context<'_>) -> Result<(), HandlerError> {
    /// let my_run_retry_policy = RunRetryPolicy::default()
    ///     .initial_delay(Duration::from_millis(100))
    ///     .exponentiation_factor(2.0)
    ///     .max_delay(Duration::from_millis(1000))
    ///     .max_attempts(10)
    ///     .max_duration(Duration::from_secs(10));
    /// ctx.run(|| write_to_other_system())
    ///     .retry_policy(my_run_retry_policy)
    ///     .await
    ///     .map_err(|e| {
    ///         // Handle the terminal error after retries exhausted
    ///         // For example, undo previous actions (see sagas guide) and
    ///         // propagate the error back to the caller
    ///         e
    ///      })?;
    /// # Ok(())
    /// # }
    /// # async fn write_to_other_system() -> Result<String, HandlerError>{
    /// # Ok("Hello".to_string())
    /// # }
    /// ```
    ///
    /// This way you can override the default retry behavior of your Restate service for specific operations.
    /// Have a look at [`RunFuture::retry_policy`] for more information.
    ///
    /// If you set a maximum number of attempts, then the `ctx.run` block will fail with a [TerminalError] once the retries are exhausted.
    /// Have a look at the [Sagas guide](https://docs.restate.dev/guides/sagas) to learn how to undo previous actions of the handler to keep the system in a consistent state.
    ///
    /// **Caution: Immediately await journaled actions:**
    /// Always immediately await `ctx.run`, before doing any other context calls.
    /// If not, you might bump into non-determinism errors during replay,
    /// because the journaled result can get interleaved with the other context calls in the journal in a non-deterministic way.
    ///
    #[must_use]
    fn run<R, F, T>(&self, run_closure: R) -> impl RunFuture<Result<T, TerminalError>> + 'ctx
    where
        R: RunClosure<Fut = F, Output = T> + Send + 'ctx,
        F: Future<Output = HandlerResult<T>> + Send + 'ctx,
        T: Serialize + Deserialize + 'static,
    {
        self.inner_context().run(run_closure)
    }

    /// Return a random seed inherently predictable, based on the invocation id, which is not secret.
    ///
    /// This value is stable during the invocation lifecycle, thus across retries.
    fn random_seed(&self) -> u64 {
        private::SealedContext::random_seed(self)
    }

    /// ### Generating random numbers
    ///
    /// Return a [`rand::Rng`] instance inherently predictable, seeded with [`ContextSideEffects::random_seed`].
    ///
    /// For example, you can use this to generate a random number:
    ///
    /// ```rust,no_run
    /// # use restate_sdk::prelude::*;
    /// # use rand::Rng;
    /// async fn rand_generate(mut ctx: Context<'_>) {
    /// let x: u32 = ctx.rand().gen();
    /// # }
    /// ```
    ///
    /// This instance is useful to generate identifiers, idempotency keys, and for uniform sampling from a set of options.
    /// If a cryptographically secure value is needed, please generate that externally using [`ContextSideEffects::run`].
    #[cfg(feature = "rand")]
    fn rand(&mut self) -> &mut rand::prelude::StdRng {
        private::SealedContext::rand(self)
    }
    /// ### Generating UUIDs
    ///
    /// Returns a random [`uuid::Uuid`], generated using [`ContextSideEffects::rand`].
    ///
    /// You can use these UUIDs to generate stable idempotency keys, to deduplicate operations. For example, you can use this to let a payment service avoid duplicate payments during retries.
    ///
    /// Do not use this in cryptographic contexts.
    ///
    /// ```rust,no_run
    /// # use restate_sdk::prelude::*;
    /// # use uuid::Uuid;
    /// # async fn uuid_generate(mut ctx: Context<'_>) {
    /// let uuid: Uuid = ctx.rand_uuid();
    /// # }
    /// ```
    #[cfg(all(feature = "rand", feature = "uuid"))]
    fn rand_uuid(&mut self) -> uuid::Uuid {
        let rand = private::SealedContext::rand(self);
        uuid::Uuid::from_u64_pair(rand::RngCore::next_u64(rand), rand::RngCore::next_u64(rand))
    }
}

impl<'ctx, CTX: private::SealedContext<'ctx>> ContextSideEffects<'ctx> for CTX {}

/// # Reading state
/// You can store key-value state in Restate.
/// Restate makes sure the state is consistent with the processing of the code execution.
///
/// **This feature is only available for Virtual Objects and Workflows:**
/// - For **Virtual Objects**, the state is isolated per Virtual Object and lives forever (across invocations for that object).
/// - For **Workflows**, you can think of it as if every workflow execution is a new object. So the state is isolated to a single workflow execution. The state can only be mutated by the `run` handler of the workflow. The other handlers can only read the state.
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// #
/// # async fn my_handler(ctx: ObjectContext<'_>) -> Result<(), HandlerError> {
///     /// 1. Listing state keys
///     /// List all the state keys that have entries in the state store for this object, via:
///     let keys = ctx.get_keys().await?;
///
///     /// 2. Getting a state value
///     let my_string = ctx
///         .get::<String>("my-key")
///         .await?
///         .unwrap_or(String::from("my-default"));
///     let my_number = ctx.get::<f64>("my-number-key").await?.unwrap_or_default();
///
///     /// 3. Setting a state value
///     ctx.set("my-key", String::from("my-value"));
///
///     /// 4. Clearing a state value
///     ctx.clear("my-key");
///
///     /// 5. Clearing all state values
///     /// Deletes all the state in Restate for the object
///     ctx.clear_all();
/// #    Ok(())
/// # }
/// ```
///
/// For more info about serialization, see [crate::serde]
///
/// ### Command-line introspection
/// You can inspect and edit the K/V state stored in Restate via `psql` and the CLI.
/// Have a look at the [introspection docs](https://docs.restate.dev//operate/introspection#inspecting-application-state) for more information.
///
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

/// # Writing State
/// You can store key-value state in Restate.
/// Restate makes sure the state is consistent with the processing of the code execution.
///
/// **This feature is only available for Virtual Objects and Workflows:**
/// - For **Virtual Objects**, the state is isolated per Virtual Object and lives forever (across invocations for that object).
/// - For **Workflows**, you can think of it as if every workflow execution is a new object. So the state is isolated to a single workflow execution. The state can only be mutated by the `run` handler of the workflow. The other handlers can only read the state.
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// #
/// # async fn my_handler(ctx: ObjectContext<'_>) -> Result<(), HandlerError> {
///     /// 1. Listing state keys
///     /// List all the state keys that have entries in the state store for this object, via:
///     let keys = ctx.get_keys().await?;
///
///     /// 2. Getting a state value
///     let my_string = ctx
///         .get::<String>("my-key")
///         .await?
///         .unwrap_or(String::from("my-default"));
///     let my_number = ctx.get::<f64>("my-number-key").await?.unwrap_or_default();
///
///     /// 3. Setting a state value
///     ctx.set("my-key", String::from("my-value"));
///
///     /// 4. Clearing a state value
///     ctx.clear("my-key");
///
///     /// 5. Clearing all state values
///     /// Deletes all the state in Restate for the object
///     ctx.clear_all();
/// #    Ok(())
/// # }
/// ```
///
/// For more info about serialization, see [crate::serde]
///
/// ## Command-line introspection
/// You can inspect and edit the K/V state stored in Restate via `psql` and the CLI.
/// Have a look at the [introspection docs](https://docs.restate.dev//operate/introspection#inspecting-application-state) for more information.
///
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
    ) -> impl DurableFuture<Output = Result<T, TerminalError>> + 'ctx {
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

pub trait DurableFuture: Future + macro_support::SealedDurableFuture {}

pub(crate) mod private {
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
