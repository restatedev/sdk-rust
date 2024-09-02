//! # Restate Rust SDK
//!
//! [Restate](https://restate.dev/) is a system for easily building resilient applications using _distributed durable async/await_.
//! This crate is the Restate SDK for writing Restate services using Rust.
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
//! For a general overview about Restate, check out the [Restate documentation](https://docs.restate.dev).

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

/// Entry-point macro to define a Restate [Workflow](https://docs.restate.dev/concepts/services#workflows).
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
/// #[restate_sdk::workflow]
/// trait Billing {
///     async fn run() -> Result<u64, TerminalError>;
///     #[shared]
///     async fn get_status() -> Result<String, TerminalError>;
/// }
/// ```
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
