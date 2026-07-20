use restate_sdk::prelude::*;

struct MutSelf;

// Handlers must take `&self`, not `&mut self`.
#[service]
impl MutSelf {
    #[handler]
    async fn greet(&mut self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(name)
    }
}

fn main() {}
