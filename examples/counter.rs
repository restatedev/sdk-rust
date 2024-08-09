use futures::future::BoxFuture;
use restate_sdk::context::ObjectContext;
use restate_sdk::discovery;
use restate_sdk::endpoint::ContextInternal;
use restate_sdk::prelude::*;
use restate_sdk::service::{Discoverable, Service};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;


// TODO end result
// #[restate::service]
// trait Counter {
//     async fn get(
//         &self,
//         ctx: ObjectContext,
//         key: &str,
//     ) -> HandlerResult<String>;
//     async fn sleep(ctx: ObjectContext) -> HandlerResult<()>;
// }

trait Counter: Sized {
    fn get(
        &self,
        ctx: ObjectContext,
        key: &str,
    ) -> impl Future<Output = HandlerResult<String>> + Send;
    fn sleep(ctx: ObjectContext) -> impl Future<Output = HandlerResult<()>> + Send;

    fn serve(self) -> ServeCounter<Self> {
        ServeCounter {
            service: Arc::new(self),
        }
    }
}

#[derive(Clone)]
pub struct ServeCounter<S> {
    service: Arc<S>,
}

struct CounterImpl;

impl Counter for CounterImpl {
    async fn get(&self, ctx: ObjectContext<'_>, key: &str) -> HandlerResult<String> {
        Ok(ctx
            .get(key)
            .await?
            .map(|v| String::from_utf8(v).unwrap())
            .unwrap_or("Unknown".to_owned()))
    }

    async fn sleep(ctx: ObjectContext<'_>) -> HandlerResult<()> {
        ctx.sleep(Duration::from_secs(30)).await?;
        Ok(())
    }
}

impl<S> Service for ServeCounter<S>
where
    S: Counter + Send + Sync + 'static,
{
    type Future = BoxFuture<'static, ()>;

    fn handle(&self, ctx: ContextInternal) -> Self::Future {
        let service_clone = Arc::clone(&self.service);
        Box::pin(async move {
            match ctx.handler_name() {
                "get" => {
                    ctx.input().await;

                    let fut = S::get(&service_clone, (&ctx).into(), "bla");

                    let res = fut.await;

                    ctx.write_output(res.map(|s| s.into_bytes()));

                    ctx.end();
                }
                "sleep" => {
                    ctx.input().await;

                    let res = S::sleep((&ctx).into()).await;

                    ctx.write_output(res.map(|_| Default::default()));

                    ctx.end();
                }
                _ => {
                    todo!()
                }
            }
        })
    }
}

impl<T: Counter> Discoverable for ServeCounter<T> {
    fn discover() -> discovery::Service {
        discovery::Service {
            ty: discovery::ServiceType::VirtualObject,
            name: discovery::ServiceName::try_from("Counter".to_string())
                .expect("Service name valid"),
            handlers: vec![
                discovery::Handler {
                    name: discovery::HandlerName::try_from("sleep").expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                },
                discovery::Handler {
                    name: discovery::HandlerName::try_from("get").expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                },
            ],
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HyperServer::new(
        Endpoint::builder()
            .with_service(CounterImpl.serve())
            .build(),
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
