use restate_sdk::prelude::*;

#[restate_sdk::handler]
pub async fn my_handler(_ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
    Ok(format!("{greeting}!"))
}

// Defines `MyService` (+ `MyServiceClient`, used to call this service from another handler).
service!(MyService: { my_handler });
