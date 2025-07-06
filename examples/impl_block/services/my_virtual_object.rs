use restate_sdk::prelude::*;

pub struct MyVirtualObject;

#[restate_sdk::object(vis = "pub(crate)")]
impl MyVirtualObject {
    #[handler]
    async fn my_handler(
        &self,
        ctx: ObjectContext<'_>,
        greeting: String,
    ) -> Result<String, HandlerError> {
        Ok(format!("Greetings {} {}", greeting, ctx.key()))
    }

    #[handler(shared)]
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
            .bind(MyVirtualObject.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
