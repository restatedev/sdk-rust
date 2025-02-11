use restate_test_utils::test_utils::TestContainer;
use restate_sdk::{discovery::{self, Service}, prelude::*};

// Should compile
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

#[tokio::test]
async fn test_container_image() {

    let mut test_container = TestContainer::new("docker.io/restatedev/restate".to_string(), "latest".to_string()).await.unwrap();

    let endpoint = Endpoint::builder()
                    .bind(MyServiceImpl.serve())
                    .build();

    test_container.serve_endpoint(endpoint).await;

    // optionally insert a delays via tokio sleep
    TestContainer::delay(1000).await;

    // optionally call invoke on service handlers
    use restate_sdk::service::Discoverable;
    let my_service:Service = ServeMyService::<MyServiceImpl>::discover();
    let invoke_response = test_container.invoke(my_service, "my_handler".to_string()).await;

    assert!(invoke_response.is_ok());

    println!("invoke response:");
    println!("{}", invoke_response.unwrap().text().await.unwrap());

}
