// This file exercises the deprecated trait-based API for back-compatibility coverage.
#![allow(deprecated)]

use restate_sdk::prelude::*;

// Should compile
#[service]
trait MyService {
    async fn my_handler(input: String) -> HandlerResult<String>;

    async fn no_input() -> HandlerResult<String>;

    async fn no_output() -> HandlerResult<()>;

    async fn no_input_no_output() -> HandlerResult<()>;

    async fn std_result() -> Result<(), std::io::Error>;

    async fn std_result_with_terminal_error() -> Result<(), TerminalError>;

    async fn std_result_with_handler_error() -> Result<(), HandlerError>;
}

#[object]
trait MyObject {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

#[workflow]
trait MyWorkflow {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

#[service]
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
