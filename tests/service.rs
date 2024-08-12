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
