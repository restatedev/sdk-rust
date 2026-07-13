use restate_sdk::prelude::*;

// Generates `MyServiceClient` (to call this service) and the `MyService::server` builder.
restate_sdk::interface! {
    service MyService {
        my_handler(String) -> String;
    }
}

#[restate_sdk::handler]
pub async fn my_handler(_ctx: Context<'_>, greeting: String) -> Result<String, HandlerError> {
    Ok(format!("{greeting}!"))
}
