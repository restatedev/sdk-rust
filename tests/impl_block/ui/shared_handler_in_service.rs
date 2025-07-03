use restate_sdk::prelude::*;

struct SharedHandlerInService;

#[restate_sdk::service]
impl SharedHandlerInService {
    #[handler(shared)]
    async fn my_handler(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .with_service(SharedHandlerInService.serve())
            .build(),
    )
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
