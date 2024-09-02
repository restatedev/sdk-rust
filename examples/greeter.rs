use restate_sdk::prelude::*;
use std::convert::Infallible;

#[restate_sdk::service]
trait Greeter {
    async fn greet(name: String) -> Result<String, Infallible>;
}

struct GreeterImpl;

impl Greeter for GreeterImpl {
    async fn greet(&self, _: Context<'_>, name: String) -> Result<String, Infallible> {
        Ok(format!("Greetings {name}"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .with_service(GreeterImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
