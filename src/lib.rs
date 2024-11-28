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
//! # SDK Overview
//!
//! The Restate Rust SDK lets you implement durable handlers. Handlers can be part of three types of services:
//!
//! Let's have a look at how to define them.
//!
//! ```rust,no_run
//! // The prelude contains all the imports you need to get started
//! use restate_sdk::prelude::*;
//!
//! // Define the service using Rust traits
//! #[restate_sdk::service]
//! trait Greeter {
//!     async fn greet(name: String) -> HandlerResult<String>;
//! }
//!
//! // Implement the service
//! struct GreeterImpl;
//! impl Greeter for GreeterImpl {
//!     async fn greet(&self, _: Context<'_>, name: String) -> HandlerResult<String> {
//!         Ok(format!("Greetings {name}"))
//!     }
//! }
//!
//! // Start the HTTP server to expose services
//! #[tokio::main]
//! async fn main() {
//!     HttpServer::new(
//!         Endpoint::builder()
//!             .bind(GreeterImpl.serve())
//!             .build(),
//!     )
//!     .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!     .await;
//! }
//! ```
//!
//! ## Service types
//! Learn more about each service type:
//! - [Service](restate_sdk_macros::service)
//! - [Virtual Object](object)
//! - [Workflow](workflow)
//!
//! ## Features
//!
//! Have a look at the following SDK capabilities:
//!
//! - [Service Communication][crate::context::ContextClient]: Durable RPC and messaging between services (optionally with a delay).
//! - [Journaling Results][crate::context::ContextSideEffects]: Persist results in Restate's log to avoid re-execution on retries
//! - State: [read][crate::context::ContextReadState] and [write](crate::context::ContextWriteState): Store and retrieve state in Restate's key-value store
//! - [Scheduling & Timers][crate::context::ContextTimers]: Let a handler pause for a certain amount of time. Restate durably tracks the timer across failures.
//! - [Awakeables][crate::context::ContextAwakeables]: Durable Futures to wait for events and the completion of external tasks.
//! - [Error Handling][crate::errors]: Restate retries failures infinitely. Use `TerminalError` to stop retries.
//! - [Serialization][crate::serde]: The SDK serializes results to send them to the Server.
//! - [Serving][crate::http_server]: Start an HTTP server to expose services.
//!
//!
//! ### Logging
//!
//! You can set the logging level of the Rust SDK via the `RESTATE_LOGGING` environment variable and the level values can be `TRACE`, `DEBUG`, `INFO`, `WARN` or `ERROR`.
//! The default log level is `INFO`.
//!
//! If you set the level to `TRACE`, you can also get more verbose logging of the journal by setting the environment variable `RESTATE_JOURNAL_LOGGING=TRACE`.
//!
//! # References
//!
//! For a general overview about Restate, check out the [Restate documentation](https://docs.restate.dev).
//!
//! You can find more Rust examples in the [examples repo](https://github.com/restatedev/examples)
//!

pub mod endpoint;
pub mod service;

pub mod context;
pub mod discovery;
pub mod errors;
#[cfg(feature = "http_server")]
pub mod http_server;
#[cfg(feature = "hyper")]
pub mod hyper;
pub mod serde;

/// Entry-point macro to define a Restate [Service](https://docs.restate.dev/concepts/services#services-1).
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::service]
/// trait Greeter {
///     async fn greet(name: String) -> Result<String, HandlerError>;
/// }
/// ```
///
/// This macro accepts a `trait` as input, and generates as output:
///
/// * A trait with the same name, that you should implement on your own concrete type (e.g. `struct`), e.g.:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # #[restate_sdk::service]
/// # trait Greeter {
/// #    async fn greet(name: String) -> Result<String, HandlerError>;
/// # }
/// struct GreeterImpl;
/// impl Greeter for GreeterImpl {
///     async fn greet(&self, _: Context<'_>, name: String) -> Result<String, HandlerError> {
///         Ok(format!("Greetings {name}"))
///     }
/// }
/// ```
///
/// This trait will additionally contain, for each handler, the appropriate [`Context`](crate::prelude::Context), to interact with Restate.
///
/// * An implementation of the [`Service`](crate::service::Service) trait, to bind the service in the [`Endpoint`](crate::prelude::Endpoint) and expose it:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # #[restate_sdk::service]
/// # trait Greeter {
/// #    async fn greet(name: String) -> HandlerResult<String>;
/// # }
/// # struct GreeterImpl;
/// # impl Greeter for GreeterImpl {
/// #    async fn greet(&self, _: Context<'_>, name: String) -> HandlerResult<String> {
/// #        Ok(format!("Greetings {name}"))
/// #    }
/// # }
/// let endpoint = Endpoint::builder()
///     // .serve() returns the implementation of Service used by the SDK
///     //  to bind your struct to the endpoint
///     .bind(GreeterImpl.serve())
///     .build();
/// ```
///
/// * A client implementation to call this service from another service, object or workflow, e.g.:
///
/// ```rust,no_run
/// # use restate_sdk::prelude::*;
/// # #[restate_sdk::service]
/// # trait Greeter {
/// #    async fn greet(name: String) -> HandlerResult<String>;
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
/// Methods of this trait can accept either no parameter, or one parameter implementing [`Deserialize`](crate::serde::Deserialize).
/// The return value MUST always be a `Result`. Down the hood, the error type is always converted to [`HandlerError`](crate::prelude::HandlerError) for the SDK to distinguish between terminal and retryable errors. For more details, check the [`HandlerError`](crate::prelude::HandlerError) doc.
///
/// When invoking the service through Restate, the method name should be used as handler name, that is:
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::service]
/// trait Greeter {
///     async fn my_greet(name: String) -> Result<String, HandlerError>;
/// }
/// ```
///
/// The `Greeter/my_greet` handler be invoked sending a request to `http://<RESTATE_ENDPOINT>/Greeter/my_greet`.
/// You can override the names used by Restate during registration using the `name` attribute:
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::service]
/// #[name = "greeter"]
/// trait Greeter {
///     // You can invoke this handler with `http://<RESTATE_ENDPOINT>/greeter/myGreet`
///     #[name = "myGreet"]
///     async fn my_greet(name: String) -> Result<String, HandlerError>;
/// }
/// ```
pub use restate_sdk_macros::service;

/// Entry-point macro to define a Restate [Virtual object](https://docs.restate.dev/concepts/services#virtual-objects).
///
/// For more details, check the [`service` macro](macro@crate::service) documentation.
///
/// ## Shared handlers
///
/// To define a shared handler, simply annotate the handler with the `#[shared]` annotation:
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::object]
/// trait Counter {
///     async fn add(val: u64) -> Result<u64, TerminalError>;
///     #[shared]
///     async fn get() -> Result<u64, TerminalError>;
/// }
/// ```
pub use restate_sdk_macros::object;

///
/// # Workflows
///
/// Entry-point macro to define a Restate [Workflow](https://docs.restate.dev/concepts/services#workflows).
///
/// Workflows are a sequence of steps that gets executed durably.
/// A workflow can be seen as a special type of [Virtual Object](https://docs.restate.dev/concepts/services#virtual-objects) with some special characteristics:
///
/// - Each workflow definition has a `run` handler that implements the workflow logic.
/// - The `run` handler executes exactly one time for each workflow instance (object / key).
/// - A workflow definition can implement other handlers that can be called multiple times, and can interact with the workflow.
/// - Workflows have access to the `WorkflowContext` and `SharedWorkflowContext`, giving them some extra functionality, for example Durable Promises to signal workflows.
///
/// **Note: Workflow retention time**:
/// The retention time of a workflow execution is 24 hours after the finishing of the `run` handler.
/// After this timeout any [K/V state][crate::context::ContextReadState] is cleared, the workflow's shared handlers cannot be called anymore, and the Durable Promises are discarded.
/// The retention time can be configured via the [Admin API](https://docs.restate.dev//references/admin-api/#tag/service/operation/modify_service) per Workflow definition by setting `workflow_completion_retention`.
///
/// ## Implementing workflows
/// Have a look at the code example to get a better understanding of how workflows are implemented:
///
/// ```
/// use restate_sdk::prelude::*;
///
/// #[restate_sdk::workflow]
/// pub trait SignupWorkflow {
///     async fn run(req: String) -> Result<bool, HandlerError>;
///     #[shared]
///     async fn click(click_secret: String) -> Result<(), HandlerError>;
///     #[shared]
///     async fn get_status() -> Result<String, HandlerError>;
/// }
///
/// pub struct SignupWorkflowImpl;
///
/// impl SignupWorkflow for SignupWorkflowImpl {
///     async fn run(&self, mut ctx: WorkflowContext<'_>, email: String) -> Result<bool, HandlerError> {
///
///         let secret = ctx.rand_uuid().to_string();
///         ctx.run(|| send_email_with_link(email, secret)).await?;
///         ctx.set("status", "Email sent".to_string());
///
///         let click_secret = ctx.promise::<String>("email.clicked").await?;
///         ctx.set("status", "Email clicked".to_string());
///
///         Ok(click_secret == secret)
///     }
///     async fn click(&self, ctx: SharedWorkflowContext<'_>, click_secret: String) -> Result<(), HandlerError> {
///         ctx.resolve_promise::<String>("email.clicked", click_secret);
///         Ok(())
///     }
///     async fn get_status(&self, ctx: SharedWorkflowContext<'_>) -> Result<String, HandlerError> {
///         Ok(ctx.get("status").unwrap_or("unknown".to_string()))
///     }
/// }
/// # fn send_email_with_link(email: String, secret: String) -> Result<(), HandlerError> {
/// #    Ok(())
/// # }
///
/// #[tokio::main]
/// async fn main() {
///     tracing_subscriber::fmt::init();
///     HttpServer::new(Endpoint::builder().bind(SignupWorkflowImpl.serve()).build())
///         .listen_and_serve("0.0.0.0:9080".parse().unwrap())
///         .await;
/// }
/// ```
///
/// ### The run handler
///
/// Every workflow needs a `run` handler.
/// This handler has access to the same SDK features as Service and Virtual Object handlers.
/// In the example above, we use [`ctx.run`][crate::context::ContextSideEffects] to log the sending of the email in Restate and avoid re-execution on replay.
///
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
///     In the example, the `click` handler gets called when the user clicks a link in an email and resolves the `email.clicked` promise.
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

/// Prelude contains all the useful imports you need to get started with Restate.
pub mod prelude {
    #[cfg(feature = "http_server")]
    pub use crate::http_server::HttpServer;

    pub use crate::context::{
        Context, ContextAwakeables, ContextClient, ContextPromises, ContextReadState,
        ContextSideEffects, ContextTimers, ContextWriteState, HeaderMap, ObjectContext, Request,
        RunFuture, RunRetryPolicy, SharedObjectContext, SharedWorkflowContext, WorkflowContext,
    };
    pub use crate::endpoint::Endpoint;
    pub use crate::errors::{HandlerError, HandlerResult, TerminalError};
    pub use crate::serde::Json;
}
