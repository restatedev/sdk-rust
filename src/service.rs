use crate::context::Context;
use crate::endpoint::HandlerContext;
use crate::errors::TerminalError;
use futures::future::BoxFuture;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub type HandlerResult<T> = Result<T, TerminalError>;

trait World: Sized {
    async fn hello(&self, context: Context, name: String) -> HandlerResult<String>;

    fn serve(self) -> ServeWorld<Self> {
        ServeWorld {
            service: Arc::new(self),
        }
    }
}

#[derive(Clone)]
pub struct ServeWorld<S> {
    service: Arc<S>,
}

impl<S> Service for ServeWorld<S>
where
    S: World,
{
    type Future = BoxFuture<'static, ()>;

    fn handle(&self, ctx: HandlerContext) -> Self::Future {
        Box::pin(async { () })
    }
}

// TODO the user writes the trait using async fn, and it gets converted to this one with Send bounds!

trait Counter: Sized {
    fn get(&self, ctx: Context, key: &str) -> impl Future<Output = HandlerResult<String>> + Send;
    fn sleep(ctx: Context) -> impl Future<Output = HandlerResult<()>> + Send;

    fn serve(self) -> ServeCounter<Self> {
        ServeCounter {
            service: Arc::new(self),
        }
    }
}

struct CounterImpl;

impl Counter for CounterImpl {
    async fn get(&self, ctx: Context<'_>, key: &str) -> HandlerResult<String> {
        Ok(ctx
            .get(key)
            .await?
            .map(|v| String::from_utf8(v).unwrap())
            .unwrap_or("Unknown".to_owned()))
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

#[derive(Clone)]
pub struct ServeCounter<S> {
    service: Arc<S>,
}

// Codegeneration output
impl<S> Service for ServeCounter<S>
where
    S: Counter + Send + Sync + 'static,
{
    type Future = BoxFuture<'static, ()>;

    fn handle(&self, ctx: HandlerContext) -> Self::Future {
        let service_clone = Arc::clone(&self.service);
        Box::pin(async move {
            match ctx.handler_name() {
                "get" => {
                    ctx.input().await;

                    let user_ctx = Context(ctx.clone(), Default::default());

                    let fut = S::get(&service_clone, user_ctx, "bla");
                    tokio::pin!(fut);

                    let res = fut.await;

                    ctx.write_output(res.map(|s| s.into_bytes()));

                    ctx.consume_to_end()
                }
                "sleep" => {
                    ctx.input().await;

                    let user_ctx = Context(ctx.clone(), Default::default());
                    let res = S::sleep(user_ctx).await;

                    ctx.write_output(res.map(|_| Default::default()));

                    ctx.consume_to_end()
                }
                _ => {
                    todo!()
                }
            }
        })
    }
}

impl<T: Counter> Discoverable for T {
    fn discover() -> crate::discovery::Service {
        crate::discovery::Service {
            ty: crate::discovery::ServiceType::VirtualObject,
            name: crate::discovery::ServiceName::try_from("Counter".to_string())
                .expect("Service name valid"),
            handlers: vec![
                crate::discovery::Handler {
                    name: crate::discovery::HandlerName::try_from("sleep")
                        .expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                },
                crate::discovery::Handler {
                    name: crate::discovery::HandlerName::try_from("get")
                        .expect("Handler name valid"),
                    input: None,
                    output: None,
                    ty: None,
                },
            ],
        }
    }
}
