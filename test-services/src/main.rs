mod awakeable_holder;
mod block_and_wait_workflow;
mod cancel_test;
mod counter;
mod failing;
mod kill_test;
mod list_object;
mod map_object;
mod non_deterministic;
mod proxy;
mod test_utils_service;
mod virtual_object_command_interpreter;

use restate_sdk::prelude::{Endpoint, HttpServer};
use std::env;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port = env::var("PORT").ok().unwrap_or("9080".to_string());
    let services = env::var("SERVICES").ok().unwrap_or("*".to_string());

    // Ambient state used by the Failing service (process-wide counters).
    let mut builder = Endpoint::builder().extension(failing::FailingState::default());

    if services == "*" || services.contains("Counter") {
        builder = builder.bind(counter::definition())
    }
    if services == "*" || services.contains("Proxy") {
        builder = builder.bind(proxy::definition())
    }
    if services == "*" || services.contains("MapObject") {
        builder = builder.bind(map_object::definition())
    }
    if services == "*" || services.contains("ListObject") {
        builder = builder.bind(list_object::definition())
    }
    if services == "*" || services.contains("AwakeableHolder") {
        builder = builder.bind(awakeable_holder::definition())
    }
    if services == "*" || services.contains("BlockAndWaitWorkflow") {
        builder = builder.bind(block_and_wait_workflow::definition())
    }
    if services == "*" || services.contains("CancelTestRunner") {
        builder = builder.bind(cancel_test::runner_definition())
    }
    if services == "*" || services.contains("CancelTestBlockingService") {
        builder = builder.bind(cancel_test::blocking_definition())
    }
    if services == "*" || services.contains("Failing") {
        builder = builder.bind(failing::definition())
    }
    if services == "*" || services.contains("KillTestRunner") {
        builder = builder.bind(kill_test::runner_definition())
    }
    if services == "*" || services.contains("KillTestSingleton") {
        builder = builder.bind(kill_test::singleton_definition())
    }
    if services == "*" || services.contains("NonDeterministic") {
        builder = builder.bind(non_deterministic::definition())
    }
    if services == "*" || services.contains("TestUtilsService") {
        builder = builder.bind(test_utils_service::definition())
    }
    if services == "*" || services.contains("VirtualObjectCommandInterpreter") {
        builder = builder.bind(virtual_object_command_interpreter::definition())
    }

    if let Ok(key) = env::var("E2E_REQUEST_SIGNING_ENV") {
        builder = builder.identity_key(&key).unwrap()
    }

    HttpServer::new(builder.build())
        .listen_and_serve(format!("0.0.0.0:{port}").parse().unwrap())
        .await;
}
