use restate_sdk::prelude::*;

struct ReservedHandler;

#[service]
impl ReservedHandler {
    #[handler]
    async fn from_client(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
