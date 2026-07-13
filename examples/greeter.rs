use restate_sdk::prelude::*;
use std::convert::Infallible;

#[restate_sdk::handler]
async fn greet(_ctx: Context<'_>, name: String) -> Result<String, Infallible> {
    Ok(format!("Greetings {name}"))
}

#[tokio::main]
async fn main() {
    // To enable logging/tracing
    // tracing_subscriber::fmt::init();
    let greeter = define_service("Greeter").handler(greet).build();
    HttpServer::new(Endpoint::builder().bind(greeter).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
