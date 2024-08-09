pub mod endpoint;
pub mod service;

pub mod context;
pub mod discovery;
pub mod errors;
#[cfg(feature = "http")]
pub mod http;
pub mod serde;

pub mod prelude {
    //pub use restate_sdk_derive::*;

    #[cfg(feature = "http")]
    pub use crate::http::HyperServer;

    pub use crate::context::Context;
    pub use crate::endpoint::Endpoint;
    pub use crate::errors::{HandlerResult, TerminalError};
    pub use crate::serde::Json;

    // TODO
    // * Macros
}
