use reqwest::StatusCode;
use restate_sdk::prelude::*;
use restate_sdk_test_env::TestContainer;
use tracing::info;

// Should compile
pub(crate) struct MyObject;

#[allow(dead_code)]
#[restate_sdk::object(vis = "pub(crate)")]
impl MyObject {
    #[handler]
    async fn my_handler(&self, _ctx: ObjectContext<'_>, _input: String) -> HandlerResult<String> { unimplemented!() }

    #[handler(shared)]
    async fn my_shared_handler(&self, _ctx: SharedObjectContext<'_>, _input: String) -> HandlerResult<String> { unimplemented!() }
}

pub(crate) struct MyWorkflow;

#[allow(dead_code)]
#[restate_sdk::workflow(vis = "pub(crate)")]
impl MyWorkflow {
    #[handler]
    async fn my_handler(&self, _ctx: WorkflowContext<'_>, _input: String) -> HandlerResult<String> { unimplemented!() }

    #[handler(shared)]
    async fn my_shared_handler(&self, _ctx: SharedWorkflowContext<'_>, _input: String) -> HandlerResult<String> { unimplemented!() }
}

pub(crate) struct MyService;

#[restate_sdk::service(vis = "pub(crate)")]
impl MyService {
    #[handler]
    async fn my_handler(&self, _ctx: Context<'_>) -> HandlerResult<String> {
        let result = "hello!";
        Ok(result.to_string())
    }
}

#[tokio::test]
async fn test_container() {
    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::INFO) // Set the maximum log level
        .init();

    let endpoint = Endpoint::builder().bind(MyService.serve()).build();

    // simple test container intialization with default configuration
    //let test_container = TestContainer::default().start(endpoint).await.unwrap();

    // custom test container initialization with builder
    let test_container = TestContainer::builder()
        // optional passthrough logging from the resstate server testcontainer
        // prints container logs to tracing::info level
        .with_container_logging()
        .with_container(
            "docker.io/restatedev/restate".to_string(),
            "latest".to_string(),
        )
        .build()
        .start(endpoint)
        .await
        .unwrap();

    let ingress_url = test_container.ingress_url();

    // call container ingress url for /MyService/my_handler
    let response = reqwest::Client::new()
        .post(format!("{}/MyService/my_handler", ingress_url))
        .header("Accept", "application/json")
        .header("Content-Type", "*/*")
        .header("idempotency-key", "abc")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    info!(
        "/MyService/my_handler response: {:?}",
        response.text().await.unwrap()
    );
}
