use crate::endpoint::HandlerContext;
use crate::errors::TerminalError;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use futures::future::BoxFuture;
use crate::context::Context;

pub type HandlerResult<T> = Result<T, TerminalError>;



trait Counter {
    async fn get(&self, ctx: Context, key: &str) -> HandlerResult<String>;
    async fn sleep(ctx: Context) -> HandlerResult<()>;
}

struct CounterImpl;

impl Counter for CounterImpl {
    async fn get(&self, ctx: Context<'_>, key: &str) -> HandlerResult<String> {
        Ok(ctx.get(key).await?.map(|v| String::from_utf8(v).unwrap()).unwrap_or("Unknown".to_owned()))
    }

    async fn sleep(ctx: Context<'_>) -> HandlerResult<()> {
        ctx.sleep(Duration::from_secs(30)).await?;
        Ok(())
    }
}

// --- codegen targets these 2 traits Trait

pub trait Service {
    type Future: Future<Output = ()> + Send + 'static;

    fn handle(&self, req: HandlerContext) -> Self::Future;
}

pub trait Discoverable {
    fn discover() -> crate::discovery::Service;
}

// --- TODO everything below is output of code generation

// Codegeneration output
impl<T: Counter + Sync + Send> Service for Arc<T> {
    type Future = BoxFuture<'static, ()>;

    fn handle(&self, ctx: HandlerContext) -> Self::Future {
        match ctx.method_name() {
            "get" => {
                let cloned = self.clone();
                Box::pin(async move {
                    ctx.input().await;

                    let user_ctx = Context(ctx.clone(), Default::default());

                    let fut = Counter::get(&*cloned, user_ctx, "bla");
                    tokio::pin!(fut);

                    let res = fut.await;

                    ctx.write_output(res.map(|s| s.into_bytes()));

                    ctx.consume_to_end()
                })
            },
            "sleep" => {
                Box::pin(async move {
                    ctx.input().await;

                    let user_ctx = Context(ctx.clone(), Default::default());
                    let res = <CounterImpl as Counter>::sleep(user_ctx).await;

                    ctx.write_output(res.map(|_| Default::default()));

                    ctx.consume_to_end()
                })
            }
            _ => {
                todo!()
            }
        }
    }
}

impl<T: Counter> Discoverable for T {
    fn discover() -> crate::discovery::Service {
        crate::discovery::Service {
            ty: crate::discovery::ServiceType::VirtualObject,
            name: crate::discovery::ServiceName::try_from("Counter".to_string()).expect("Service name valid"),
            handlers: vec![
                crate::discovery::Handler {
                    name: crate::discovery::HandlerName::try_from("sleep").expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                },
                crate::discovery::Handler {
                    name: crate::discovery::HandlerName::try_from("get").expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                }
            ],
        }
    }
}
