use restate_sdk::prelude::*;
use std::collections::HashMap;

#[restate_sdk::service]
trait RunExample {
    async fn do_run() -> Result<Json<HashMap<String, String>>, HandlerError>;
}

struct RunExampleImpl(reqwest::Client);

impl RunExample for RunExampleImpl {
    async fn do_run(
        &self,
        context: Context<'_>,
    ) -> Result<Json<HashMap<String, String>>, HandlerError> {
        let res = context
            .run(|| async move {
                let req = self.0.get("https://httpbin.org/ip").build()?;

                let res = self
                    .0
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
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(
        Endpoint::builder()
            .bind(RunExampleImpl(reqwest::Client::new()).serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
