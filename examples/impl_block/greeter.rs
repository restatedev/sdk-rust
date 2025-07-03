use restate_sdk::prelude::*;
use std::convert::Infallible;

struct Greeter;

#[restate_sdk::service]
impl Greeter {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>, name: String) -> Result<String, Infallible> {
        Ok(format!("Greetings {name}"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(Greeter.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
