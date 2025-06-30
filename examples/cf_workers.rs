#![cfg(target_family = "wasm")]

// this is an example of a Rust file that uses the restate-sdk crate to create a Cloudflare Worker service.
// this code needs to be compiled to WebAssembly using the wrangler dev tools with extra rust flags to enable wasm support
//
// for local development use this command:
// RUSTFLAGS='--cfg getrandom_backend="wasm_js"' npx wrangler dev
//
// or push to production using this command:
// RUSTFLAGS='--cfg getrandom_backend="wasm_js"' npx wrangler deploy
//
// The Cloudflare Worker automated build pipeline doesn't currently support this code due to missing clang binaries
//

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
