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
        builder = builder.with_service(counter::Counter::serve(counter::CounterImpl))
    }
    if services == "*" || services.contains("Proxy") {
        builder = builder.with_service(proxy::Proxy::serve(proxy::ProxyImpl))
    }
    if services == "*" || services.contains("MapObject") {
        builder = builder.with_service(map_object::MapObject::serve(map_object::MapObjectImpl))
    }
    if services == "*" || services.contains("ListObject") {
        builder = builder.with_service(list_object::ListObject::serve(list_object::ListObjectImpl))
    }
    if services == "*" || services.contains("AwakeableHolder") {
        builder = builder.with_service(awakeable_holder::AwakeableHolder::serve(
            awakeable_holder::AwakeableHolderImpl,
        ))
    }
    if services == "*" || services.contains("BlockAndWaitWorkflow") {
        builder = builder.with_service(block_and_wait_workflow::BlockAndWaitWorkflow::serve(
            block_and_wait_workflow::BlockAndWaitWorkflowImpl,
        ))
    }
    if services == "*" || services.contains("CancelTestRunner") {
        builder = builder.with_service(cancel_test::CancelTestRunner::serve(
            cancel_test::CancelTestRunnerImpl,
        ))
    }
    if services == "*" || services.contains("CancelTestBlockingService") {
        builder = builder.with_service(cancel_test::CancelTestBlockingService::serve(
            cancel_test::CancelTestBlockingServiceImpl,
        ))
    }
    if services == "*" || services.contains("Failing") {
        builder = builder.with_service(failing::Failing::serve(failing::FailingImpl::default()))
    }
    if services == "*" || services.contains("KillTestRunner") {
        builder = builder.with_service(kill_test::KillTestRunner::serve(
            kill_test::KillTestRunnerImpl,
        ))
    }
    if services == "*" || services.contains("KillTestSingleton") {
        builder = builder.with_service(kill_test::KillTestSingleton::serve(
            kill_test::KillTestSingletonImpl,
        ))
    }
    if services == "*" || services.contains("NonDeterministic") {
        builder = builder.with_service(non_deterministic::NonDeterministic::serve(
            non_deterministic::NonDeterministicImpl::default(),
        ))
    }
    if services == "*" || services.contains("TestUtilsService") {
        builder = builder.with_service(test_utils_service::TestUtilsService::serve(
            test_utils_service::TestUtilsServiceImpl,
        ))
    }

    if let Ok(key) = env::var("E2E_REQUEST_SIGNING_ENV") {
        builder = builder.with_identity_key(&key).unwrap()
    }

    HttpServer::new(builder.build())
        .listen_and_serve(format!("0.0.0.0:{port}").parse().unwrap())
        .await;
}
