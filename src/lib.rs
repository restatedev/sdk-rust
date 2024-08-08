mod endpoint;
pub mod service;

// TODO Add feature
mod context;
mod discovery;
mod errors;
pub mod http;

mod prelude {
    //pub use restate_sdk_derive::*;

    // TODO
    // * Macros
    // * HTTP server (if feature enabled)
    // * Context
    // * Maybe Service trait?
}
