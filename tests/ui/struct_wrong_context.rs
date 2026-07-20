use restate_sdk::prelude::*;

struct WrongContext;

// A `#[service]` handler must use `Context`, not `ObjectContext`.
#[restate_sdk::service]
impl WrongContext {
    #[handler]
    async fn greet(&self, _ctx: ObjectContext<'_>, name: String) -> HandlerResult<String> {
        Ok(name)
    }
}

fn main() {}
