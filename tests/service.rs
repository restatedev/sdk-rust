use restate_sdk::prelude::*;

// Should compile
#[restate_sdk::service]
trait MyService {
    async fn my_handler(input: String) -> HandlerResult<String>;

    async fn no_input() -> HandlerResult<String>;

    async fn no_output() -> HandlerResult<()>;

    async fn no_input_no_output() -> HandlerResult<()>;
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

#[restate_sdk::service]
#[name = "myRenamedService"]
trait MyRenamedService {
    #[name = "myRenamedHandler"]
    async fn my_handler() -> HandlerResult<()>;
}

struct MyRenamedServiceImpl;

impl MyRenamedService for MyRenamedServiceImpl {
    async fn my_handler(&self, _: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn renamed_service_handler() {
    use restate_sdk::service::Discoverable;

    let discovery = ServeMyRenamedService::<MyRenamedServiceImpl>::discover();
    assert_eq!(discovery.name.to_string(), "myRenamedService");
    assert_eq!(discovery.handlers[0].name.to_string(), "myRenamedHandler");
}
