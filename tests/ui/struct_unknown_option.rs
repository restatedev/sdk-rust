use restate_sdk::prelude::*;

struct UnknownOption;

// `bogus` is not a recognized handler configuration option.
#[service]
impl UnknownOption {
    #[handler(bogus = "x")]
    async fn greet(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
