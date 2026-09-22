use restate_sdk::ingress::ReqwestClient;
use restate_sdk::prelude::*;
use restate_sdk_testcontainers::TestEnvironment;

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

    // Service
    let service_client = MyServiceIngressClient::from_client(client.clone());
    let response = service_client
        .my_handler()
        .idempotency_key("abc")
        .call()
        .await
        .unwrap();
    let output = response.into_body().unwrap();
    assert_eq!(output, "hello!");

    // Virtual object: exclusive and shared handlers on the same key.
    let object_client = MyObjectIngressClient::from_client(client.clone(), "my-object-key");
    let object_output = object_client
        .my_handler("object input".to_owned())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    assert_eq!(object_output, "object input");

    let shared_output = object_client
        .my_shared_handler("shared input".to_owned())
        .call()
        .await
        .unwrap()
        .into_body()
        .unwrap();
    assert_eq!(shared_output, "shared input");

    // Workflow: submit one-way, then resolve the invocation via handle() and attach for its result.
    let workflow_client = MyWorkflowIngressClient::from_client(client, "my-workflow-key");
    workflow_client
        .my_handler("workflow input".to_owned())
        .send()
        .await
        .unwrap();

    let handle = workflow_client.handle().await.unwrap();
    let workflow_output = handle.attach().await.unwrap().into_body();
    assert_eq!(workflow_output, "workflow input");
}
