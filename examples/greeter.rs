use restate_sdk::prelude::*;
use std::time::Duration;
use tracing::info;

#[restate_sdk::service]
trait Greeter {
    async fn sleep() -> Result<(), HandlerError>;
    async fn do_stuff() -> Result<String, HandlerError>;
}

struct GreeterImpl;

impl Greeter for GreeterImpl {
    async fn sleep(&self, ctx: Context<'_>) -> Result<(), HandlerError> {
        ctx.sleep(Duration::from_secs(120)).await?;
        Ok(())
    }

    async fn do_stuff(&self, ctx: Context<'_>) -> Result<String, HandlerError> {
        let call_handle = ctx.service_client::<GreeterClient>()
            .sleep()
            .call();

        info!("Invocation id {}", call_handle.invocation_id().await?);

        ctx.sleep(Duration::from_secs(5)).await?;

        // Now cancel it
        call_handle.cancel();

        // Now await it
        call_handle.await?;

        Ok("This should have failed".to_string())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(GreeterImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
