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
//!             .with_service(GreeterImpl.serve())
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

pub use restate_sdk_macros::{object, service, workflow};

/// Prelude contains all the useful imports you need to get started with Restate.
pub mod prelude {
    #[cfg(feature = "http_server")]
    pub use crate::http_server::HttpServer;

    pub use crate::context::{
        Context, ContextAwakeables, ContextClient, ContextPromises, ContextReadState,
        ContextSideEffects, ContextTimers, ContextWriteState, HeaderMap, ObjectContext, Request,
        RunRetryPolicy, SharedObjectContext, SharedWorkflowContext, WorkflowContext,
    };
    pub use crate::endpoint::Endpoint;
    pub use crate::errors::{HandlerError, HandlerResult, TerminalError};
    pub use crate::serde::Json;
}
