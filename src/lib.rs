pub mod endpoint;
pub mod service;

pub mod context;
pub mod discovery;
pub mod errors;
#[cfg(feature = "http_server")]
pub mod http;
pub mod serde;

pub use restate_sdk_macros::{object, service, workflow};

pub mod prelude {
    #[cfg(feature = "http_server")]
    pub use crate::http::HyperServer;

    pub use crate::context::{
        Context, ContextAwakeables, ContextClient, ContextPromises, ContextReadState,
        ContextSideEffects, ContextTimers, ContextWriteState, HeaderMap, ObjectContext, Request,
        SharedObjectContext, SharedWorkflowContext, WorkflowContext,
    };
    pub use crate::endpoint::Endpoint;
    pub use crate::errors::{HandlerError, HandlerResult, TerminalError};
    pub use crate::serde::Json;
}
