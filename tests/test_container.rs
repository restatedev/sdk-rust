use restate_test_utils::test_utils::TestContainer;
use restate_sdk::{discovery::Service, prelude::*};

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

    let test_container = TestContainer::new().await.unwrap();

    // uses higher port number to avoid collisions
    // with non-test instances running locally
    let host_port:u16 = 19080;
    let host_address = format!("0.0.0.0:{}", host_port);

    println!("starting host");
    // boot restate server
    tokio::spawn(async move {
        HttpServer::new(
            Endpoint::builder()
                .bind(MyServiceImpl.serve())
                .build(),
        ).listen_and_serve(host_address.parse().unwrap()).await;
    });

    use restate_sdk::service::Discoverable;
    let my_service:Service = ServeMyService::<MyServiceImpl>::discover();

    let registered = test_container.register(host_port).await;

    TestContainer::delay(1000).await;

    let response = test_container.invoke(my_service, "my_handler".to_string()).await;

    assert!(registered.is_ok());

}
