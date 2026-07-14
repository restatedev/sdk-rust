use restate_sdk::prelude::*;
use std::convert::Infallible;

#[restate_sdk::handler]
async fn greet(_ctx: Context<'_>, name: String) -> Result<String, Infallible> {
    Ok(format!("Greetings {name}"))
}

// Declaratively define the `Greeter` service (and a `GreeterClient`) from the handler(s).
service!(Greeter: { greet });

#[tokio::main]
async fn main() {
    // To enable logging/tracing
    // tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(Greeter).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
