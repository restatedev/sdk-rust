use reqwest::StatusCode;
use restate_sdk::prelude::*;
use restate_sdk_testcontainers::TestEnvironment;
use tracing::info;

#[restate_sdk::service]
trait MyService {
    async fn my_handler() -> HandlerResult<String>;
}

#[restate_sdk::object]
trait MyObject {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

#[restate_sdk::workflow]
trait MyWorkflow {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

struct MyServiceImpl;

impl MyService for MyServiceImpl {
    async fn my_handler(&self, _: Context<'_>) -> HandlerResult<String> {
        let result = "hello!";
        Ok(result.to_string())
    }
}

async fn start_test_environment() -> restate_sdk_testcontainers::StartedTestEnvironment {
    let _ = tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let endpoint = Endpoint::builder().bind(MyServiceImpl.serve()).build();

    TestEnvironment::new()
        .with_container_logging()
        .with_container(
            "docker.io/restatedev/restate".to_string(),
            "latest".to_string(),
        )
        .start(endpoint)
        .await
        .unwrap()
}

#[tokio::test]
async fn test_container() {
    let test_environment = start_test_environment().await;
    let ingress_url = test_environment.ingress_url();
    // call container ingress url for /MyService/my_handler
    let response = reqwest::Client::new()
        .post(format!("{}/MyService/my_handler", ingress_url))
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

#[cfg(feature = "ingress-client")]
#[tokio::test]
async fn test_container_ingress_client() {
    let test_environment = start_test_environment().await;

    let client = restate_sdk::ingress::Client::new(
        test_environment.ingress_url().try_into().unwrap(),
        None,
    )
    .unwrap();

    let response = client
        .service_client::<MyServiceClient>()
        .my_handler()
        .idempotency_key("abc")
        .call()
        .await
        .unwrap();

    assert_eq!(response, "hello!");
    info!("/MyService/my_handler response: {:?}", response);
}
