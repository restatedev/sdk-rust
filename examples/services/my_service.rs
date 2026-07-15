use restate_sdk::prelude::*;

#[restate_sdk::handler]
pub async fn my_handler(_ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
    Ok(format!("{greeting}!"))
}

// Defines MyService
service!(MyService: { my_handler });
