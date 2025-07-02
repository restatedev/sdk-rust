#![cfg(target_family = "wasm")]

// test requires wasm-pack / wasm-bindgen-test framework
// use following command to execute test:
// RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack test --node -- --no-default-features --features cf_workers --test cf_workers
use restate_sdk::{cf_workers::CfWorkerServer, prelude::*};
use wasm_bindgen_test::*;

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

#[wasm_bindgen_test]
fn cf_workerservice_handler() {
    let endpoint = Endpoint::builder()
        .bind(MyServiceImpl.serve())
        .build();

    let cf_server = CfWorkerServer::new(endpoint);
    let health_check_request =  http::Request::builder().uri("/health").body(worker::Body::empty()).unwrap();
    let result = cf_server.call(health_check_request);
}
