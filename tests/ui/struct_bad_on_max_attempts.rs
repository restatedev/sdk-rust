use restate_sdk::prelude::*;

struct BadOnMaxAttempts;

// `on_max_attempts` only accepts "pause" or "kill".
#[service]
impl BadOnMaxAttempts {
    #[handler(invocation_retry_policy(max_attempts = 3, on_max_attempts = "explode"))]
    async fn greet(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
