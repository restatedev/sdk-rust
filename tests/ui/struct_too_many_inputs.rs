use restate_sdk::prelude::*;

struct TooManyInputs;

// Handlers support at most one input argument (after the context).
#[service]
impl TooManyInputs {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>, a: String, b: String) -> HandlerResult<String> {
        Ok(a + &b)
    }
}

fn main() {}
