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

use restate_sdk::prelude::{Endpoint, HttpServer};
use std::env;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port = env::var("PORT").ok().unwrap_or("9080".to_string());
    let services = env::var("SERVICES").ok().unwrap_or("*".to_string());

    let mut builder = Endpoint::builder();

    if services == "*" || services.contains("Counter") {
        builder = builder.bind(counter::Counter::serve(counter::CounterImpl))
    }
    if services == "*" || services.contains("Proxy") {
        builder = builder.bind(proxy::Proxy::serve(proxy::ProxyImpl))
    }
    if services == "*" || services.contains("MapObject") {
        builder = builder.bind(map_object::MapObject::serve(map_object::MapObjectImpl))
    }
    if services == "*" || services.contains("ListObject") {
        builder = builder.bind(list_object::ListObject::serve(list_object::ListObjectImpl))
    }
    if services == "*" || services.contains("AwakeableHolder") {
        builder = builder.bind(awakeable_holder::AwakeableHolder::serve(
            awakeable_holder::AwakeableHolderImpl,
        ))
    }
    if services == "*" || services.contains("BlockAndWaitWorkflow") {
        builder = builder.bind(block_and_wait_workflow::BlockAndWaitWorkflow::serve(
            block_and_wait_workflow::BlockAndWaitWorkflowImpl,
        ))
    }
    if services == "*" || services.contains("CancelTestRunner") {
        builder = builder.bind(cancel_test::CancelTestRunner::serve(
            cancel_test::CancelTestRunnerImpl,
        ))
    }
    if services == "*" || services.contains("CancelTestBlockingService") {
        builder = builder.bind(cancel_test::CancelTestBlockingService::serve(
            cancel_test::CancelTestBlockingServiceImpl,
        ))
    }
    if services == "*" || services.contains("Failing") {
        builder = builder.bind(failing::Failing::serve(failing::FailingImpl::default()))
    }
    if services == "*" || services.contains("KillTestRunner") {
        builder = builder.bind(kill_test::KillTestRunner::serve(
            kill_test::KillTestRunnerImpl,
        ))
    }
    if services == "*" || services.contains("KillTestSingleton") {
        builder = builder.bind(kill_test::KillTestSingleton::serve(
            kill_test::KillTestSingletonImpl,
        ))
    }
    if services == "*" || services.contains("NonDeterministic") {
        builder = builder.bind(non_deterministic::NonDeterministic::serve(
            non_deterministic::NonDeterministicImpl::default(),
        ))
    }
    if services == "*" || services.contains("TestUtilsService") {
        builder = builder.bind(test_utils_service::TestUtilsService::serve(
            test_utils_service::TestUtilsServiceImpl,
        ))
    }

    if let Ok(key) = env::var("E2E_REQUEST_SIGNING_ENV") {
        builder = builder.identity_key(&key).unwrap()
    }

    HttpServer::new(builder.build())
        .listen_and_serve(format!("0.0.0.0:{port}").parse().unwrap())
        .await;
}
