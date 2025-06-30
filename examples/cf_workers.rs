#![cfg(target_family = "wasm")]

use restate_sdk::prelude::*;

#[restate_sdk::service]
trait MyService {
    async fn my_handler() -> HandlerResult<()>;
}

struct MyServiceImpl;

impl MyService for MyServiceImpl {
    async fn my_handler(&self, _: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[worker::event(fetch)]
pub async fn main( req:worker::HttpRequest, _env: worker::Env, _ctx: worker::Context) -> worker::Result<http::Response<worker::Body>> {
    let endpoint = Endpoint::builder()
        .with_protocol_mode(restate_sdk::discovery::ProtocolMode::RequestResponse) // Cloud
        .bind(MyServiceImpl.serve())
        .build();

    let cf_worker = CfWorkerServer::new(endpoint);

    return cf_worker.call(req).await;
}
