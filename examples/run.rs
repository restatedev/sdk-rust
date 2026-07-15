use restate_sdk::prelude::*;
use std::collections::HashMap;

#[restate_sdk::handler]
async fn do_run(ctx: Context<'_>) -> Result<Json<HashMap<String, String>>, HandlerError> {
    // The HTTP client is injected as ambient state (see `main`), instead of living on a struct.
    let client = ctx.extension::<reqwest::Client>().clone();
    let res = ctx
        .run(|| async move {
            let req = client.get("https://httpbin.org/ip").build()?;

            let res = client
                .execute(req)
                .await?
                .json::<HashMap<String, String>>()
                .await?;

            Ok(Json::from(res))
        })
        .name("get_ip")
        .await?
        .into_inner();

    Ok(res.into())
}

service!(RunExample: { do_run });

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    // The HTTP client is injected as a service-scoped extension.
    let run_example = RunExample.with_extension(reqwest::Client::new());
    HttpServer::new(Endpoint::builder().bind(run_example).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
