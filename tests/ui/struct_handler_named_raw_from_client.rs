use restate_sdk::prelude::*;

struct RawReservedHandler;

#[service]
impl RawReservedHandler {
    #[handler]
    async fn r#from_client(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
