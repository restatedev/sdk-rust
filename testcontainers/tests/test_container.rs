use restate_sdk::ingress::ReqwestClient;
use restate_sdk::prelude::*;
use restate_sdk_testcontainers::TestEnvironment;
use tracing::info;

struct MyService;

#[service]
impl MyService {
    #[handler]
    async fn my_handler(&self, _ctx: Context<'_>) -> HandlerResult<String> {
        let result = "hello!";
        Ok(result.to_string())
    }
}

struct MyObject;

#[object]
impl MyObject {
    #[handler]
    async fn my_handler(&self, _ctx: ObjectContext<'_>, input: String) -> HandlerResult<String> {
        Ok(input)
    }

    #[handler]
    async fn my_shared_handler(
        &self,
        _ctx: SharedObjectContext<'_>,
        input: String,
    ) -> HandlerResult<String> {
        Ok(input)
    }
}

struct MyWorkflow;

#[workflow]
impl MyWorkflow {
    #[handler]
    async fn my_handler(&self, _ctx: WorkflowContext<'_>, input: String) -> HandlerResult<String> {
        Ok(input)
    }

    #[handler]
    async fn my_shared_handler(
        &self,
        _ctx: SharedWorkflowContext<'_>,
        input: String,
    ) -> HandlerResult<String> {
        Ok(input)
    }
}

#[tokio::test]
async fn test_container() {
    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::INFO) // Set the maximum log level
        .init();

    let endpoint = Endpoint::builder()
        .bind(MyService)
        .bind(MyObject)
        .bind(MyWorkflow)
        .build();

    // simple test container initialization with default configuration
    //let test_container = TestContainer::default().start(endpoint).await.unwrap();

    // custom test container initialization with builder
    let test_environment = TestEnvironment::new()
        // optional passthrough logging from the restate server testcontainers
        // prints container logs to tracing::info level
        .with_container_logging()
        .with_container(
            "docker.io/restatedev/restate".to_string(),
            "latest".to_string(),
        )
        .start(endpoint)
        .await
        .unwrap();

    let ingress_url = test_environment.ingress_url();

    let client = ReqwestClient::connect(ingress_url.parse().unwrap()).unwrap();
    let client = MyServiceIngressClient::from_client(client);

    let response = client
        .my_handler()
        .idempotency_key("abc")
        .call()
        .await
        .unwrap();
    let output = response.into_body().unwrap();

    assert_eq!(output, "hello!");
    info!("MyService/my_handler response: {output:?}");
}
