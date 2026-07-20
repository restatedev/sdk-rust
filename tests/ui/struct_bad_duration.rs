use restate_sdk::prelude::*;

struct BadDuration;

// The duration string is not parseable by jiff's friendly format.
#[service(inactivity_timeout = "not-a-duration")]
impl BadDuration {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
