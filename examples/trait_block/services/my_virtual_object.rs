use restate_sdk::prelude::*;

#[restate_sdk::object]
pub trait MyVirtualObject {
    async fn my_handler(name: String) -> Result<String, HandlerError>;
    #[shared]
    async fn my_concurrent_handler(name: String) -> Result<String, HandlerError>;
}

pub struct MyVirtualObjectImpl;

impl MyVirtualObject for MyVirtualObjectImpl {
    async fn my_handler(
        &self,
        ctx: ObjectContext<'_>,
        greeting: String,
    ) -> Result<String, HandlerError> {
        Ok(format!("Greetings {} {}", greeting, ctx.key()))
    }
    async fn my_concurrent_handler(
        &self,
        ctx: SharedObjectContext<'_>,
        greeting: String,
    ) -> Result<String, HandlerError> {
        Ok(format!("Greetings {} {}", greeting, ctx.key()))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .bind(MyVirtualObjectImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
