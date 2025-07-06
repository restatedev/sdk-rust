use restate_sdk::prelude::*;

pub struct MyService;

#[restate_sdk::service(vis = "pub(crate)")]
impl MyService {
    #[handler]
    async fn my_handler(&self, _ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
        Ok(format!("{greeting}!"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(MyService.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
