//! # Restate Rust SDK
//!
//! [Restate](https://restate.dev/) is a system for easily building resilient applications.
//! This crate is the Restate SDK for writing Restate services using Rust.
//!
//! ## New to Restate?
//!
//! If you are new to Restate, we recommend the following resources:
//!
//! - [Learn about the concepts of Restate](https://docs.restate.dev/concepts/durable_building_blocks)
//! - Use cases:
//!     - [Workflows](https://docs.restate.dev/use-cases/workflows)
//!     - [Microservice orchestration](https://docs.restate.dev/use-cases/microservice-orchestration)
//!     - [Event processing](https://docs.restate.dev/use-cases/event-processing)
//!     - [Async tasks](https://docs.restate.dev/use-cases/async-tasks)
//! - [Quickstart](https://docs.restate.dev/get_started/quickstart?sdk=rust)
//! - [Do the Tour of Restate to try out the APIs](https://docs.restate.dev/get_started/tour/?sdk=rust)
//!
//! # Features
//!
//! Have a look at the following SDK capabilities:
//!
//! - [SDK Overview](#sdk-overview): Overview of the SDK and how to implement services, virtual objects, and workflows.
//! - [Service Communication][crate::context::ContextClient]: Durable RPC and messaging between services (optionally with a delay).
//! - [Journaling Results][crate::context::ContextSideEffects]: Persist results in Restate's log to avoid re-execution on retries
//! - State: [read][crate::context::ContextReadState] and [write](crate::context::ContextWriteState): Store and retrieve state in Restate's key-value store
//! - [Scheduling & Timers][crate::context::ContextTimers]: Let a handler pause for a certain amount of time. Restate durably tracks the timer across failures.
//! - [Awakeables][crate::context::ContextAwakeables]: Durable Futures to wait for events and the completion of external tasks.
//! - [Error Handling][crate::errors]: Restate retries failures infinitely. Use `TerminalError` to stop retries.
//! - [Serialization][crate::serde]: The SDK serializes results to send them to the Server. Includes [Schema Generation and payload metadata](crate::serde::PayloadMetadata) for documentation & discovery.
//! - [Dependency injection][crate::context::ContextExtensions]: Inject dependencies such as HTTP clients, or DB Pools, into your handlers.
//! - [Serving][crate::http_server]: Start an HTTP server to expose services.
//!
//! # SDK Overview
//!
//! The Restate Rust SDK lets you implement durable handlers. Handlers can be part of three types of services:
//!
//! - [Services](https://docs.restate.dev/concepts/services/#services-1): a collection of durable handlers
//! - [Virtual Objects](https://docs.restate.dev/concepts/services/#virtual-objects): an object consists of a collection of durable handlers and isolated K/V state. Virtual Objects are useful for modeling stateful entities, where at most one handler can run at a time per object.
//! - [Workflows](https://docs.restate.dev/concepts/services/#workflows): Workflows have a `run` handler that executes exactly once per workflow instance, and executes a set of steps durably. Workflows can have other handlers that can be called multiple times and interact with the workflow.
//!
//! ## Services
//!
//! [Services](https://docs.restate.dev/concepts/services/#services-1) and their handlers are defined as follows:
//!
//! ```rust,no_run
//! // The prelude contains all the imports you need to get started
//! use restate_sdk::prelude::*;
//!
//! // Handlers are plain async functions annotated with #[restate_sdk::handler].
//! // The first parameter is a context; `Context` marks this as a Service handler.
//! #[restate_sdk::handler]
//! async fn my_handler(ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
//!     Ok(format!("{greeting}!"))
//! }
//!
//! // Declaratively define the `MyService` service
//! service!(MyService: { my_handler });
//!
//! // Start the HTTP server to expose services
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(Endpoint::builder().bind(MyService).build())
//!         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!         .await;
//! }
//! ```
//!
//! - A handler is an `async fn` annotated with [`#[restate_sdk::handler]`](macro@crate::handler). Its first parameter is [`Context`](crate::context::Context) which allows you to use Restate features.
//! - It takes an optional single input and returns a `Result` with the output value, or a [`HandlerError`](crate::errors::HandlerError). Input and output types implement the SDK's [`Serialize`](crate::serde::Serialize)/[`Deserialize`](crate::serde::Deserialize) (see [`crate::serde`]); returning a [`TerminalError`](crate::errors::TerminalError) rather than any other error stops retries.
//! - Compose handlers into a service with `service!`, then bind it to an [`Endpoint`](crate::endpoint::Endpoint) and serve. It's invocable at `<INGRESS_URL>/MyService/my_handler`.
//!
//! ## Virtual Objects
//! [Virtual Objects](https://docs.restate.dev/concepts/services/#virtual-objects) and their handlers are defined similarly to services, with the following differences:
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//!
//! // An exclusive handler: takes ObjectContext, can read and write the object's K/V state.
//! #[restate_sdk::handler]
//! async fn my_handler(ctx: ObjectContext<'_>, greeting: String) -> Result<String, HandlerError> {
//!     Ok(format!("{} {}", greeting, ctx.key()))
//! }
//!
//! // A shared handler: takes SharedObjectContext. Being "shared" is inferred from the context type.
//! #[restate_sdk::handler]
//! async fn my_concurrent_handler(ctx: SharedObjectContext<'_>, greeting: String) -> Result<String, HandlerError> {
//!     Ok(format!("{} {}", greeting, ctx.key()))
//! }
//!
//! object!(MyVirtualObject: { my_handler, my_concurrent_handler });
//!
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(Endpoint::builder().bind(MyVirtualObject).build())
//!         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!         .await;
//! }
//! ```
//!
//! - Same handler shape as a service; compose them with `object!`. The context type sets each handler's role:
//! - [`ObjectContext`](crate::context::ObjectContext) — exclusive read/write access to the object's K/V state; at most one such handler runs at a time per key ([`ctx.key()`](crate::context::ObjectContext::key) gives the key).
//! - [`SharedObjectContext`](crate::context::SharedObjectContext) — runs concurrently with read-only state access (e.g. to expose state or resolve awakeables). "Shared" is inferred from the context type — no `#[shared]`.
//!
//! ## Workflows
//!
//! [Workflows](https://docs.restate.dev/concepts/services/#workflows) are a special type of Virtual Objects, their definition is similar but with the following differences:
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//!
//! // The `run` handler: takes WorkflowContext and executes exactly once per workflow id.
//! #[restate_sdk::handler]
//! async fn run(ctx: WorkflowContext<'_>, req: String) -> Result<String, HandlerError> {
//!     // implement workflow logic here
//!     Ok(String::from("success"))
//! }
//!
//! // Additional handlers interact with the workflow; they take a SharedWorkflowContext.
//! #[restate_sdk::handler]
//! async fn interact_with_workflow(ctx: SharedWorkflowContext<'_>) -> Result<(), HandlerError> {
//!     // implement interaction logic here
//!     // e.g. resolve a promise that the workflow is waiting on
//!     Ok(())
//! }
//!
//! workflow!(MyWorkflow: { run, interact_with_workflow });
//!
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(Endpoint::builder().bind(MyWorkflow).build())
//!         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!         .await;
//! }
//! ```
//!
//! - Same handler shape again; compose them with `workflow!`. A workflow has:
//! - one `run` handler taking a [`WorkflowContext`](crate::context::WorkflowContext), executed exactly once per workflow id;
//! - any number of handlers taking a [`SharedWorkflowContext`](crate::context::SharedWorkflowContext) to query or signal it: they run concurrently with `run` and stay callable after it finishes.
//! - See the [workflow docs](crate::prelude::workflow) for more.
//!
//!
//! Learn more about each service type here:
//! - [Service](crate::prelude::service)
//! - [Virtual Object](crate::prelude::object)
//! - [Workflow](crate::prelude::workflow)
//!
//!
//! ### Logging
//!
//! This crate uses the [tracing crate][tracing] to emit logs, so you'll need to configure a tracing subscriber to get logs. For example, to configure console logging using `tracing_subscriber::fmt`:
//! ```rust,no_run
//! #[tokio::main]
//! async fn main() {
//!     //! To enable logging
//!     tracing_subscriber::fmt::init();
//!
//!     // Start http server etc...
//! }
//! ```
//!
//! You can filter logs *when a handler is being replayed* configuring the [filter::ReplayAwareFilter].
//!
//! For more information about tracing and logging, have a look at the [tracing subscriber doc](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/index.html#filtering-events-with-environment-variables).
//!
//! Next, have a look at the other [SDK features](#features).
//!

pub mod endpoint;
pub mod service;

pub mod context;
pub mod discovery;
pub mod errors;
pub(crate) mod extensions;
#[cfg(feature = "tracing-span-filter")]
pub mod filter;
#[cfg(feature = "http_server")]
pub mod http_server;
#[cfg(feature = "hyper")]
pub mod hyper;
#[cfg(feature = "lambda")]
pub mod lambda;
pub mod serde;

/// Entry-point macro to define a Restate [Service](https://docs.restate.dev/concepts/services#services-1).
///
/// <div class="warning">
///
/// **Deprecated.** Prefer the function-first API: annotate handlers with
/// [`#[restate_sdk::handler]`](macro@crate::handler) and compose them with
/// `service!`/`object!`/`workflow!`. **This macro will be removed in the next releases**.
///
/// </div>
#[deprecated]
pub use restate_sdk_macros::service;

/// Entry-point macro to define a Restate [Virtual object](https://docs.restate.dev/concepts/services#virtual-objects).
///
/// <div class="warning">
///
/// **Deprecated.** Prefer the function-first API: annotate handlers with
/// [`#[restate_sdk::handler]`](macro@crate::handler) and compose them with
/// `service!`/`object!`/`workflow!`. **This macro will be removed in the next releases**.
///
/// </div>
#[deprecated]
pub use restate_sdk_macros::object;

///
/// # Workflows
///
/// Entry-point macro to define a Restate [Workflow](https://docs.restate.dev/concepts/services#workflows).
///
/// <div class="warning">
///
/// **Deprecated.** Prefer the function-first API: annotate handlers with
/// [`#[restate_sdk::handler]`](macro@crate::handler) and compose them with
/// `service!`/`object!`/`workflow!`. **This macro will be removed in the next releases**.
///
/// </div>
#[deprecated]
pub use restate_sdk_macros::workflow;

/// Turn a free `async fn` into a composable Restate handler.
///
/// The function's first parameter is a Restate context, whose type selects the service kind:
/// [`Context`](crate::context::Context) → service;
/// [`ObjectContext`](crate::context::ObjectContext) / [`SharedObjectContext`](crate::context::SharedObjectContext) → virtual object;
/// [`WorkflowContext`](crate::context::WorkflowContext) / [`SharedWorkflowContext`](crate::context::SharedWorkflowContext) → workflow.
/// Compose the annotated handlers into a service with [`service!`](crate::prelude::service),
/// [`object!`](crate::prelude::object) or [`workflow!`](crate::prelude::workflow) — see those macros
/// for how to define, bind and call a service.
///
/// The handler's wire name defaults to the function name; override it with
/// `#[restate_sdk::handler(name = "myGreet")]`.
pub use restate_sdk_macros::handler;

// Declarative composition macros. They are exported from the macros crate under `define_*` names
// (which don't clash with the deprecated `#[service]`/`#[object]`/`#[workflow]` attribute macros)
// and re-exported by the `prelude` under the ergonomic names `service!`/`object!`/`workflow!`.
#[doc(hidden)]
pub use restate_sdk_macros::{define_object, define_service, define_workflow};

/// Prelude contains all the useful imports you need to get started with Restate.
pub mod prelude {
    #[cfg(feature = "http_server")]
    pub use crate::http_server::HttpServer;

    #[cfg(feature = "lambda")]
    pub use crate::lambda::LambdaEndpoint;

    pub use crate::context::{
        CallFuture, Context, ContextAwakeables, ContextClient, ContextExtensions, ContextPromises,
        ContextReadState, ContextSideEffects, ContextTimers, ContextWriteState,
        DurableFuturesUnordered, HeaderMap, InvocationHandle, ObjectContext, Request, RunFuture,
        RunRetryPolicy, SharedObjectContext, SharedWorkflowContext, WorkflowContext,
    };
    pub use crate::endpoint::{
        Endpoint, HandleOptions, HandlerOptions, ProtocolMode, ServiceOptions,
    };
    pub use crate::errors::{HandlerError, HandlerResult, TerminalError};
    pub use crate::serde::Json;

    /// Define a Restate [Service](https://docs.restate.dev/concepts/services#services-1) from a set
    /// of [`#[handler]`](macro@crate::handler) functions.
    ///
    /// ```rust,no_run
    /// use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::handler]
    /// async fn greet(ctx: Context<'_>, name: String) -> Result<String, HandlerError> {
    ///     Ok(format!("Greetings {name}"))
    /// }
    ///
    /// service!(Greeter: { greet });
    /// ```
    ///
    /// `service!(Name: { handler, .. })` takes the service name and the handlers, and generates:
    ///
    /// * A zero-sized `Greeter` type implementing [`Service`](crate::service::Service), to bind in
    ///   the [`Endpoint`](crate::prelude::Endpoint) and expose it:
    ///
    /// ```rust,no_run
    /// # use restate_sdk::prelude::*;
    /// # #[restate_sdk::handler]
    /// # async fn greet(ctx: Context<'_>, name: String) -> Result<String, HandlerError> { Ok(name) }
    /// # service!(Greeter: { greet });
    /// let endpoint = Endpoint::builder()
    ///     .bind(Greeter)
    ///     .build();
    /// ```
    ///
    /// * A client implementation to call this service from another service, object or workflow, e.g.:
    ///
    /// ```rust,no_run
    /// # use restate_sdk::prelude::*;
    /// # #[restate_sdk::handler]
    /// # async fn greet(ctx: Context<'_>, name: String) -> Result<String, HandlerError> { Ok(name) }
    /// # service!(Greeter: { greet });
    /// # async fn example(ctx: Context<'_>) -> Result<(), TerminalError> {
    /// let result = ctx
    ///    .service_client::<GreeterClient>()
    ///    .greet("My greetings".to_string())
    ///    .call()
    ///    .await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Handlers accept either no parameter, or one parameter implementing
    /// [`Deserialize`](crate::serde::Deserialize). The return value MUST always be a `Result`. Down
    /// the hood, the error type is always converted to [`HandlerError`](crate::prelude::HandlerError)
    /// for the SDK to distinguish between terminal and retryable errors. For more details, check the
    /// [`HandlerError`](crate::prelude::HandlerError) doc.
    ///
    /// The service name is the identifier passed to `service!` (`Greeter` here), and each handler is
    /// invoked at `http://<RESTATE_ENDPOINT>/Greeter/<handler>` using the function name. Override a
    /// handler's wire name with `#[restate_sdk::handler(name = "myGreet")]`.
    pub use crate::define_service as service;

    /// Define a Restate [Virtual Object](https://docs.restate.dev/concepts/services#virtual-objects)
    /// from a set of [`#[handler]`](macro@crate::handler) functions.
    ///
    /// For more details, check the [`service!`](crate::prelude::service) macro documentation.
    ///
    /// ## Shared handlers
    ///
    /// A handler is *shared* when its first parameter is a
    /// [`SharedObjectContext`](crate::context::SharedObjectContext) instead of an
    /// [`ObjectContext`](crate::context::ObjectContext) — no `#[shared]` annotation needed, it is
    /// inferred from the context type:
    ///
    /// ```rust,no_run
    /// use restate_sdk::prelude::*;
    ///
    /// #[restate_sdk::handler]
    /// async fn add(ctx: ObjectContext<'_>, val: u64) -> Result<u64, TerminalError> {
    ///     Ok(val)
    /// }
    ///
    /// #[restate_sdk::handler]
    /// async fn get(ctx: SharedObjectContext<'_>) -> Result<u64, TerminalError> {
    ///     Ok(0)
    /// }
    ///
    /// object!(Counter: { add, get });
    /// ```
    pub use crate::define_object as object;

    /// Define a Restate [Workflow](https://docs.restate.dev/concepts/services#workflows) from a set
    /// of [`#[handler]`](macro@crate::handler) functions.
    ///
    /// [Workflows](https://docs.restate.dev/concepts/services#workflows) are a sequence of steps that
    /// gets executed durably. A workflow can be seen as a special type of
    /// [Virtual Object](https://docs.restate.dev/concepts/services#virtual-objects) with the
    /// following characteristics:
    ///
    /// - Each workflow definition has a **`run` handler** (its first parameter is a
    ///   [`WorkflowContext`](crate::context::WorkflowContext)) that implements the workflow logic.
    ///     - The `run` handler **executes exactly one time** for each workflow instance (object / key).
    ///     - The `run` handler executes a set of **durable steps/activities**. These can either be:
    ///         - Inline activities: for example a [run block](crate::context::ContextSideEffects) or [sleep](crate::context::ContextTimers)
    ///         - [Calls to other handlers](crate::context::ContextClient) implementing the activities
    /// - A workflow definition can implement other handlers (taking a
    ///   [`SharedWorkflowContext`](crate::context::SharedWorkflowContext)) that can be called
    ///   multiple times, and can **interact with the workflow**:
    ///   - Query the workflow by getting K/V state or awaiting promises that are resolved by the workflow.
    ///   - Signal the workflow by resolving promises that the workflow waits on.
    /// - The K/V state of the workflow is isolated to the workflow execution, and can only be mutated by the `run` handler.
    ///
    /// **Note: Workflow retention time**:
    /// The retention time of a workflow execution is 24 hours after the finishing of the `run` handler.
    /// After this timeout any [K/V state][crate::context::ContextReadState] is cleared, the workflow's shared handlers cannot be called anymore, and the Durable Promises are discarded.
    /// The retention time can be configured via the [Admin API](https://docs.restate.dev/references/admin-api/#tag/service/operation/modify_service) per Workflow definition by setting `workflow_completion_retention`.
    ///
    /// ## Implementing workflows
    /// Have a look at the code example to get a better understanding of how workflows are implemented:
    ///
    /// ```rust,no_run
    /// use restate_sdk::prelude::*;
    ///
    /// // The `run` handler: takes a WorkflowContext and executes exactly once per workflow id.
    /// #[restate_sdk::handler]
    /// async fn run(mut ctx: WorkflowContext<'_>, email: String) -> Result<bool, HandlerError> {
    ///     let secret = ctx.rand_uuid().to_string();
    ///     ctx.run(|| send_email_with_link(email.clone(), secret.clone())).await?;
    ///     ctx.set("status", "Email sent".to_string());
    ///
    ///     let click_secret = ctx.promise::<String>("email.clicked").await?;
    ///     ctx.set("status", "Email clicked".to_string());
    ///
    ///     Ok(click_secret == secret)
    /// }
    ///
    /// // A shared handler to signal the workflow; takes a SharedWorkflowContext.
    /// #[restate_sdk::handler]
    /// async fn click(ctx: SharedWorkflowContext<'_>, click_secret: String) -> Result<(), HandlerError> {
    ///     ctx.resolve_promise::<String>("email.clicked", click_secret);
    ///     Ok(())
    /// }
    ///
    /// // A shared handler to query the workflow.
    /// #[restate_sdk::handler]
    /// async fn get_status(ctx: SharedWorkflowContext<'_>) -> Result<String, HandlerError> {
    ///     Ok(ctx.get("status").await?.unwrap_or("unknown".to_string()))
    /// }
    ///
    /// workflow!(SignupWorkflow: { run, click, get_status });
    ///
    /// # async fn send_email_with_link(email: String, secret: String) -> Result<(), HandlerError> {
    /// #    Ok(())
    /// # }
    /// #[tokio::main]
    /// async fn main() {
    ///     HttpServer::new(Endpoint::builder().bind(SignupWorkflow).build())
    ///         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    ///         .await;
    /// }
    /// ```
    ///
    /// ### The run handler
    ///
    /// Every workflow needs a `run` handler.
    /// This handler has access to the same SDK features as Service and Virtual Object handlers.
    /// In the example above, we use [`ctx.run`][crate::context::ContextSideEffects::run] to log the sending of the email in Restate and avoid re-execution on replay, or call other handlers to execute activities.
    ///
    /// ### Querying workflows
    ///
    /// Similar to Virtual Objects, you can retrieve the [K/V state][crate::context::ContextReadState] of workflows via the other (shared) handlers defined in the workflow definition.
    /// In the example we expose the status of the workflow to external clients.
    /// Every workflow execution can be seen as a new object, so the state is isolated to a single workflow execution.
    /// The state can only be mutated by the `run` handler of the workflow. The other handlers can only read the state.
    ///
    /// ### Signaling workflows
    ///
    /// You can use Durable Promises to interact with your running workflows: to let the workflow block until an event occurs, or to send a signal / information into or out of a running workflow.
    /// These promises are durable and distributed, meaning they survive crashes and can be resolved or rejected by any handler in the workflow.
    ///
    /// Do the following:
    /// 1. Create a promise that is durable and distributed in the `run` handler, and wait for its completion. In the example, we wait on the promise `email.clicked`.
    /// 2. Resolve or reject the promise in another handler in the workflow. This can be done at most one time.
    ///    In the example, the `click` handler gets called when the user clicks a link in an email and resolves the `email.clicked` promise.
    ///
    /// You can also use this pattern in reverse and let the `run` handler resolve promises that other handlers are waiting on.
    ///
    /// ### Serving and submitting workflows
    ///
    /// You serve workflows in the same way as Services and Virtual Objects — bind the generated type
    /// to the [endpoint][crate::http_server] and [register it](https://docs.restate.dev/operate/registration) in Restate.
    /// You then [**submit/query/signal**][crate::context::ContextClient] a workflow by calling its
    /// handlers like any other service (over HTTP, add `/send` for one-way calls); the `run` handler
    /// can be called (submitted) at most once per workflow ID.
    ///
    /// ```shell
    /// curl localhost:8080/SignupWorkflow/someone/run \
    ///     -H 'content-type: application/json' \
    ///     -d '"someone@restate.dev"'
    /// ```
    ///
    /// For more details on the common macro output (the bindable type + the generated client), check
    /// the [`service!`](crate::prelude::service) macro documentation.
    pub use crate::define_workflow as workflow;
}
