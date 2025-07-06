use restate_sdk::prelude::*;

#[restate_sdk::service]
trait SharedHandlerInService {
    #[shared]
    async fn my_handler() -> HandlerResult<()>;
}

struct SharedHandlerInServiceImpl;

impl SharedHandlerInService for SharedHandlerInServiceImpl {
    async fn my_handler(&self, _: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .with_service(SharedHandlerInServiceImpl.serve())
            .build(),
    )
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
