use restate_sdk::prelude::*;

#[restate_sdk::service]
pub trait MyService {
    async fn my_handler(greeting: String) -> Result<String, HandlerError>;
}

pub struct MyServiceImpl;

impl MyService for MyServiceImpl {
    async fn my_handler(&self, ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
        Ok(format!("{greeting}!"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(MyServiceImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
