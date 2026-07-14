//! `interface!` generates a typed client and a conformance-checked server builder.
//!
//! In a real project the `interface!` block (and shared request/response types) would live in a
//! separate crate that callers depend on, without importing the implementation.

use restate_sdk::prelude::*;

// The shared interface: yields `GreeterClient`, `Greeter` (+ `Greeter::server`) and `GreeterHandlers`.
restate_sdk::interface! {
    service Greeter {
        greet(String) -> String;
    }
}

// The implementation of the `greet` handler.
#[restate_sdk::handler]
async fn greet(_ctx: Context<'_>, name: String) -> HandlerResult<String> {
    Ok(format!("Hi {name}"))
}

// A second service that calls Greeter through the generated, typed client.
#[restate_sdk::handler]
async fn proxy(ctx: Context<'_>, name: String) -> HandlerResult<String> {
    let greeting = ctx
        .service_client::<GreeterClient>()
        .greet(name)
        .call()
        .await?;
    Ok(greeting)
}

// The proxy is a plain single-crate service defined with the declarative macro.
service!(Proxy: { proxy });

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Conformance-checked: `greet` must match the interface's `greet(String) -> String`.
    let greeter = Greeter::from_handlers(GreeterHandlers { greet });

    HttpServer::new(Endpoint::builder().bind(greeter).bind(Proxy).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
