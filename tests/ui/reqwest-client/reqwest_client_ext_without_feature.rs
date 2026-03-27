use restate_sdk::prelude::*;

#[restate_sdk::service]
trait LocalService {
    async fn my_handler() -> HandlerResult<String>;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ingress::Client::new("http://localhost:8080".try_into()?, None);
    let executor = reqwest::Client::new();

    let _ = client
        .service_client::<LocalServiceClient>()
        .my_handler()
        .idempotency_key("abc")
        .call(&executor);
    Ok(())
}
