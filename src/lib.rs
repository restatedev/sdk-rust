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
//! - [Configuration][crate::configuration]: Configure services, objects, workflows and their handlers — timeouts, retention, private-ness and the invocation retry policy — via attribute arguments.
//! - [Service Communication][crate::context::ContextClient]: Durable RPC and messaging between services (optionally with a delay).
//! - [Journaling Results][crate::context::ContextSideEffects]: Persist results in Restate's log to avoid re-execution on retries
//! - State: [read][crate::context::ContextReadState] and [write](crate::context::ContextWriteState): Store and retrieve state in Restate's key-value store
//! - [Scheduling & Timers][crate::context::ContextTimers]: Let a handler pause for a certain amount of time. Restate durably tracks the timer across failures.
//! - [Awakeables][crate::context::ContextAwakeables]: Durable Futures to wait for events and the completion of external tasks.
//! - [Signals][crate::context::ContextSignals]: Named durable promises scoped to an invocation, for communication between invocations.
//! - [Error Handling][crate::errors]: Restate retries failures infinitely. Use `TerminalError` to stop retries.
//! - [Serialization][crate::serde]: The SDK serializes results to send them to the Server. Includes [Schema Generation and payload metadata](crate::serde::PayloadMetadata) for documentation & discovery.
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
//! // Define the service by annotating an impl block
//! struct MyService;
//!
//! #[restate_sdk::service]
//! impl MyService {
//!     #[handler]
//!     async fn my_handler(&self, ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
//!         Ok(format!("{greeting}!"))
//!     }
//! }
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
//! - Define a service by putting the [`#[restate_sdk::service]` macro](restate_sdk_macros::service) on an `impl` block of a `struct`, and annotate each handler with [`#[handler]`](macro@crate::handler).
//!     - Handlers take `&self`, a [`Context`](crate::context::Context), and optionally one input parameter, and return a [`Result`].
//!     - The type of the input parameter of the handler needs to implement [`Serialize`](crate::serde::Deserialize) and [`Deserialize`](crate::serde::Deserialize). See [`crate::serde`].
//!     - The Result contains the return value or a [`HandlerError`][crate::errors::HandlerError], which can be a [`TerminalError`](crate::errors::TerminalError) or any other Rust's [`std::error::Error`].
//!     - The service handler can now be called at `<RESTATE_INGRESS_URL>/MyService/my_handler`. You can optionally override the handler name used via `#[handler(name = "myHandler")]`, and the service name via `#[restate_sdk::service(name = "MyService")]`. More details on handler invocations can be found in the [docs](https://docs.restate.dev/invoke/http).
//! - Store dependencies (e.g. clients, config) as fields on the `struct` and access them via `&self`. The struct is shared (behind an [`Arc`](std::sync::Arc)) across all concurrent invocations, so use interior mutability (e.g. a [`Mutex`](std::sync::Mutex) or atomics) for any mutable state.
//! - The parameter after `&self` is always a [`Context`](crate::context::Context) to interact with Restate.
//!   The SDK stores the actions you do on the context in the Restate journal to make them durable.
//! - Finally, create an HTTP endpoint and bind the service(s) to it — pass the value directly to [`bind`](crate::endpoint::Builder::bind), no `.serve()` needed. Listen on the specified port (here 9080) for connections and requests.
//!
//! ## Virtual Objects
//! [Virtual Objects](https://docs.restate.dev/concepts/services/#virtual-objects) and their handlers are defined similarly to services, with the following differences:
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//!
//! pub struct MyVirtualObject;
//!
//! #[restate_sdk::object]
//! impl MyVirtualObject {
//!     #[handler]
//!     async fn my_handler(
//!         &self,
//!         ctx: ObjectContext<'_>,
//!         greeting: String,
//!     ) -> Result<String, HandlerError> {
//!         Ok(format!("{} {}", greeting, ctx.key()))
//!     }
//!
//!     #[handler]
//!     async fn my_concurrent_handler(
//!         &self,
//!         ctx: SharedObjectContext<'_>,
//!         greeting: String,
//!     ) -> Result<String, HandlerError> {
//!         Ok(format!("{} {}", greeting, ctx.key()))
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(Endpoint::builder().bind(MyVirtualObject).build())
//!         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!         .await;
//! }
//! ```
//!
//! - Specify that you want to create a Virtual Object by putting the [`#[restate_sdk::object]` macro](restate_sdk_macros::object) on the `impl` block.
//! - The context after `&self` must be the [`ObjectContext`](crate::context::ObjectContext) parameter. Handlers with the `ObjectContext` parameter can write to the K/V state store. Only one handler can be active at a time per object, to ensure consistency.
//! - You can retrieve the key of the object you are in via [`ObjectContext.key`].
//! - If you want to have a handler that executes concurrently to the others and doesn't have write access to the K/V state, use the [`SharedObjectContext`](crate::context::SharedObjectContext) as its context.
//!   The shared/exclusive kind is inferred from the context type.
//!   You can use these handlers, for example, to read K/V state and expose it to the outside world, or to interact with the blocking handler and resolve awakeables etc.
//!
//! ## Workflows
//!
//! [Workflows](https://docs.restate.dev/concepts/services/#workflows) are a special type of Virtual Objects, their definition is similar but with the following differences:
//!
//! ```rust,no_run
//! use restate_sdk::prelude::*;
//!
//! pub struct MyWorkflow;
//!
//! #[restate_sdk::workflow]
//! impl MyWorkflow {
//!     #[handler]
//!     async fn run(&self, ctx: WorkflowContext<'_>, req: String) -> Result<String, HandlerError> {
//!         // implement workflow logic here
//!
//!         Ok(String::from("success"))
//!     }
//!
//!     #[handler]
//!     async fn interact_with_workflow(&self, ctx: SharedWorkflowContext<'_>) -> Result<(), HandlerError> {
//!         // implement interaction logic here
//!         // e.g. resolve a promise that the workflow is waiting on
//!
//!         Ok(())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(Endpoint::builder().bind(MyWorkflow).build())
//!         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!         .await;
//! }
//! ```
//!
//! - Specify that you want to create a Workflow by putting the [`#[restate_sdk::workflow]` macro](workflow) on the `impl` block.
//! - The workflow needs to have a `run` handler.
//! - The context of the `run` handler must be the [`WorkflowContext`](crate::context::WorkflowContext) parameter.
//!   The `WorkflowContext` parameter is used to interact with Restate.
//!   The `run` handler executes exactly once per workflow instance.
//! - The other handlers of the workflow are used to interact with the workflow: either query it, or signal it.
//!   They use the [`SharedWorkflowContext`](crate::context::SharedWorkflowContext) to interact with the SDK.
//!   These handlers can run concurrently with the run handler and can still be called after the run handler has finished.
//! - Have a look at the [workflow docs](workflow) to learn more.
//!
//!
//! Learn more about each service type here:
//! - [Service](restate_sdk_macros::service)
//! - [Virtual Object](object)
//! - [Workflow](workflow)
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

pub mod configuration;
pub mod context;
pub mod discovery;
pub mod errors;
#[cfg(feature = "tracing-span-filter")]
pub mod filter;
#[cfg(feature = "http_server")]
pub mod http_server;
#[cfg(feature = "hyper")]
pub mod hyper;
#[cfg(feature = "lambda")]
pub mod lambda;
pub mod serde;

/// Re-export of the [`rand`] crate this SDK depends on.
#[cfg(feature = "rand")]
pub use rand;

/// Re-export of the [`uuid`] crate this SDK depends on.
#[cfg(feature = "uuid")]
pub use uuid;

/// Entry-point macro to define a Restate [Service](https://docs.restate.dev/concepts/services#services-1).
///
/// Apply it to an inherent `impl` block of a `struct`, and annotate each handler with
/// [`#[handler]`](macro@crate::handler):
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// struct Greeter;
///
/// #[restate_sdk::service]
/// impl Greeter {
///     #[handler]
///     async fn greet(&self, _ctx: Context<'_>, name: String) -> Result<String, HandlerError> {
///         Ok(format!("Greetings {name}"))
///     }
/// }
/// ```
///
/// This macro keeps your `impl` block as-is (so handlers stay directly callable, and you can add
/// non-`#[handler]` helper methods), and additionally generates:
///
/// * An implementation of the [`Service`](crate::service::Service) and [`IntoServiceDefinition`](crate::service::IntoServiceDefinition)
///   traits, so you can bind the value directly to the [`Endpoint`](crate::prelude::Endpoint) — no `.serve()`:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # struct Greeter;
/// # #[restate_sdk::service]
/// # impl Greeter {
/// #    #[handler]
/// #    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
/// #        Ok(format!("Greetings {name}"))
/// #    }
/// # }
/// let endpoint = Endpoint::builder()
///     .bind(Greeter)
///     .build();
/// ```
///
/// * A client implementation to call this service from another service, object or workflow, e.g.:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # struct Greeter;
/// # #[restate_sdk::service]
/// # impl Greeter {
/// #    #[handler]
/// #    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
/// #        Ok(format!("Greetings {name}"))
/// #    }
/// # }
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
/// Handlers can accept either no parameter, or one parameter implementing [`Deserialize`](crate::serde::Deserialize).
/// The return value MUST always be a `Result`. Under the hood, the error type is always converted to [`HandlerError`](crate::prelude::HandlerError) for the SDK to distinguish between terminal and retryable errors. For more details, check the [`HandlerError`](crate::prelude::HandlerError) doc.
///
/// When invoking the service through Restate, the method name is used as handler name, that is the
/// `greet` handler above is invoked by sending a request to `http://<RESTATE_ENDPOINT>/Greeter/greet`.
/// You can override the names used by Restate during registration using the `name` argument:
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// struct Greeter;
///
/// #[restate_sdk::service(name = "greeter")]
/// impl Greeter {
///     // You can invoke this handler with `http://<RESTATE_ENDPOINT>/greeter/myGreet`
///     #[handler(name = "myGreet")]
///     async fn my_greet(&self, _ctx: Context<'_>, name: String) -> Result<String, HandlerError> {
///         Ok(format!("Greetings {name}"))
///     }
/// }
/// ```
///
/// The visibility of the generated client defaults to `pub`; override it with
/// `#[restate_sdk::service(client_visibility = "pub(crate)")]`.
pub use restate_sdk_macros::service;

/// Entry-point macro to define a Restate [Virtual object](https://docs.restate.dev/concepts/services#virtual-objects).
///
/// For more details, check the [`service` macro](macro@crate::service) documentation.
///
/// ## Shared handlers
///
/// To define a shared handler, use the [`SharedObjectContext`](crate::context::SharedObjectContext)
/// as the handler's context; exclusive handlers use the [`ObjectContext`](crate::context::ObjectContext).
/// The kind is inferred from the context type.
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// struct Counter;
///
/// #[restate_sdk::object]
/// impl Counter {
///     #[handler]
///     async fn add(&self, ctx: ObjectContext<'_>, val: u64) -> Result<u64, TerminalError> {
///         Ok(val)
///     }
///     #[handler]
///     async fn get(&self, ctx: SharedObjectContext<'_>) -> Result<u64, TerminalError> {
///         Ok(0)
///     }
/// }
/// ```
pub use restate_sdk_macros::object;

///
/// # Workflows
///
/// Entry-point macro to define a Restate [Workflow](https://docs.restate.dev/concepts/services#workflows).
///
/// [Workflows](https://docs.restate.dev/concepts/services#workflows) are a sequence of steps that gets executed durably.
///
/// A workflow can be seen as a special type of [Virtual Object](https://docs.restate.dev/concepts/services#virtual-objects) with the following characteristics:
///
/// - Each workflow definition has a **`run` handler** that implements the workflow logic.
///     - The `run` handler **executes exactly one time** for each workflow instance (object / key).
///     - The `run` handler executes a set of **durable steps/activities**. These can either be:
///         - Inline activities: for example a [run block](crate::context::ContextSideEffects) or [sleep](crate::context::ContextTimers)
///         - [Calls to other handlers](crate::context::ContextClient) implementing the activities
/// - You can **submit a workflow** in the same way as any handler invocation (via SDK clients or Restate services, over HTTP or Kafka).
/// - A workflow definition can implement other handlers that can be called multiple times, and can **interact with the workflow**:
///   - Query the workflow (get information out of it) by getting K/V state or awaiting promises that are resolved by the workflow.
///   - Signal the workflow (send information to it) by resolving promises that the workflow waits on.
/// - Workflows have access to the [`WorkflowContext`](crate::context::WorkflowContext) and [`SharedWorkflowContext`](crate::context::SharedWorkflowContext), giving them some extra functionality, for example [Durable Promises](#signaling-workflows) to signal workflows.
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
/// pub struct SignupWorkflow;
///
/// #[restate_sdk::workflow]
/// impl SignupWorkflow {
///     #[handler]
///     async fn run(&self, mut ctx: WorkflowContext<'_>, email: String) -> Result<bool, HandlerError> {
///         let secret = ctx.rand_uuid().to_string();
///         ctx.run(|| send_email_with_link(email.clone(), secret.clone())).await?;
///         ctx.set("status", "Email sent".to_string());
///
///         let click_secret = ctx.promise::<String>("email.clicked").await?;
///         ctx.set("status", "Email clicked".to_string());
///
///         Ok(click_secret == secret)
///     }
///
///     #[handler]
///     async fn click(&self, ctx: SharedWorkflowContext<'_>, click_secret: String) -> Result<(), HandlerError> {
///         ctx.resolve_promise::<String>("email.clicked", click_secret);
///         Ok(())
///     }
///
///     #[handler]
///     async fn get_status(&self, ctx: SharedWorkflowContext<'_>) -> Result<String, HandlerError> {
///         Ok(ctx.get("status").await?.unwrap_or("unknown".to_string()))
///     }
/// }
/// # async fn send_email_with_link(email: String, secret: String) -> Result<(), HandlerError> {
/// #    Ok(())
/// # }
///
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
/// In the example above, we use [`ctx.run`][crate::context::ContextSideEffects::run] to log the sending of the email in Restate and avoid re-execution on replay.
/// Or call other handlers to execute activities.
///
/// ## Shared handlers
///
/// To define a shared handler, simply annotate the handler with the `#[shared]` annotation:
///
/// ### Querying workflows
///
/// Similar to Virtual Objects, you can retrieve the [K/V state][crate::context::ContextReadState] of workflows via the other handlers defined in the workflow definition,
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
/// For example, the `run` handler could resolve a promise when it finishes a step of the workflow, so that other handlers can request whether this step has been completed.
///
/// ### Serving and registering workflows
///
/// You serve workflows in the same way as Services and Virtual Objects. Have a look at the [Serving docs][crate::http_server].
/// Make sure you [register the endpoint or Lambda handler](https://docs.restate.dev/operate/registration) in Restate before invoking it.
///
/// **Tip: Workflows-as-code with Restate**:
/// [Check out some examples of workflows-as-code with Restate on the use case page](https://docs.restate.dev/use-cases/workflows).
///
///
/// ## Submitting workflows from a Restate service
/// [**Submit/query/signal**][crate::context::ContextClient]:
/// Call the workflow handlers in the same way as for Services and Virtual Objects.
/// You can only call the `run` handler (submit) once per workflow ID (here `"someone"`).
/// Check out the [Service Communication docs][crate::context::ContextClient] for more information.
///
/// ## Submitting workflows over HTTP
/// [**Submit/query/signal**](https://docs.restate.dev/invoke/http#request-response-calls-over-http):
/// Call any handler of the workflow in the same way as for Services and Virtual Objects.
/// This returns the result of the handler once it has finished.
/// Add `/send` to the path for one-way calls.
/// You can only call the `run` handler once per workflow ID (here `"someone"`).
///
/// ```shell
/// curl localhost:8080/SignupWorkflow/someone/run \
///     -H 'content-type: application/json' \
///     -d '"someone@restate.dev"'
/// ```
///
/// [**Attach/peek**](https://docs.restate.dev/invoke/http#retrieve-result-of-invocations-and-workflows):
/// This lets you retrieve the result of a workflow or check if it's finished.
///
/// ```shell
/// curl localhost:8080/restate/workflow/SignupWorkflow/someone/attach
/// curl localhost:8080/restate/workflow/SignupWorkflow/someone/output
/// ```
///
/// ## Inspecting workflows
///
/// Have a look at the [introspection docs](https://docs.restate.dev/operate/introspection) on how to inspect workflows.
/// You can use this to for example:
/// - [Inspect the progress of a workflow by looking at the invocation journal](https://docs.restate.dev/operate/introspection#inspecting-the-invocation-journal)
/// - [Inspect the K/V state of a workflow](https://docs.restate.dev/operate/introspection#inspecting-application-state)
///
///
/// For more details, check the [`service` macro](macro@crate::service) documentation.
pub use restate_sdk_macros::workflow;

/// Marks a method inside a `#[restate_sdk::service]`/`#[object]`/`#[workflow]` impl block as a
/// Restate handler.
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// struct Greeter;
///
/// #[restate_sdk::service]
/// impl Greeter {
///     #[handler]
///     async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
///         Ok(format!("Greetings {name}"))
///     }
/// }
/// ```
///
/// Handlers must be `async fn`, take `&self` and a context as their first two arguments, optionally
/// a single input argument, and return `Result<T, E>` (or [`HandlerResult<T>`](crate::errors::HandlerResult)).
/// The context type selects the handler kind: [`Context`](crate::context::Context) for services,
/// [`ObjectContext`](crate::context::ObjectContext)/[`SharedObjectContext`](crate::context::SharedObjectContext)
/// for virtual objects, [`WorkflowContext`](crate::context::WorkflowContext)/[`SharedWorkflowContext`](crate::context::SharedWorkflowContext)
/// for workflows. Override the handler's Restate name with `#[handler(name = "..")]`.
pub use restate_sdk_macros::handler;

/// Prelude contains all the useful imports you need to get started with Restate.
pub mod prelude {
    #[cfg(feature = "http_server")]
    pub use crate::http_server::HttpServer;

    #[cfg(feature = "lambda")]
    pub use crate::lambda::LambdaEndpoint;

    pub use crate::context::{
        CallFuture, Context, ContextAwakeables, ContextClient, ContextPromises, ContextReadState,
        ContextSideEffects, ContextSignals, ContextTimers, ContextWriteState,
        DurableFuturesUnordered, HeaderMap, InvocationHandle, ObjectContext, Request, RunFuture,
        RunRetryPolicy, SendHandle, SharedObjectContext, SharedWorkflowContext, SignalHandle,
        WorkflowContext,
    };
    pub use crate::endpoint::{
        Endpoint, HandleOptions, HandlerOptions, ProtocolMode, ServiceOptions,
    };
    pub use crate::errors::{HandlerError, HandlerResult, TerminalError};
    pub use crate::serde::Json;
    pub use crate::service::{IntoServiceDefinition, ServiceDefinition};

    #[cfg(feature = "rand")]
    pub use rand::{Rng, RngExt};

    pub use restate_sdk_macros::{handler, object, service, workflow};
}
