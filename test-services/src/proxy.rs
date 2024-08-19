use futures::future::BoxFuture;
use futures::FutureExt;
use restate_sdk::context::RequestTarget;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyRequest {
    service_name: String,
    virtual_object_key: Option<String>,
    handler_name: String,
    message: Vec<u8>,
    delay_millis: Option<u64>,
}

impl ProxyRequest {
    fn to_target(&self) -> RequestTarget {
        if let Some(key) = &self.virtual_object_key {
            RequestTarget::Object {
                name: self.service_name.clone(),
                key: key.clone(),
                handler: self.handler_name.clone(),
            }
        } else {
            RequestTarget::Service {
                name: self.service_name.clone(),
                handler: self.handler_name.clone(),
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManyCallRequest {
    proxy_request: ProxyRequest,
    one_way_call: bool,
    await_at_the_end: bool,
}

#[restate_sdk::service]
#[name = "Proxy"]
pub(crate) trait Proxy {
    #[name = "call"]
    async fn call(req: Json<ProxyRequest>) -> HandlerResult<Json<Vec<u8>>>;
    #[name = "oneWayCall"]
    async fn one_way_call(req: Json<ProxyRequest>) -> HandlerResult<()>;
    #[name = "manyCalls"]
    async fn many_calls(req: Json<Vec<ManyCallRequest>>) -> HandlerResult<()>;
}

pub(crate) struct ProxyImpl;

impl Proxy for ProxyImpl {
    async fn call(
        &self,
        ctx: Context<'_>,
        Json(req): Json<ProxyRequest>,
    ) -> HandlerResult<Json<Vec<u8>>> {
        Ok(ctx.call(req.to_target(), req.message).await?)
    }

    async fn one_way_call(
        &self,
        ctx: Context<'_>,
        Json(req): Json<ProxyRequest>,
    ) -> HandlerResult<()> {
        ctx.send(
            req.to_target(),
            req.message,
            req.delay_millis.map(Duration::from_millis),
        );
        Ok(())
    }

    async fn many_calls(
        &self,
        ctx: Context<'_>,
        Json(requests): Json<Vec<ManyCallRequest>>,
    ) -> HandlerResult<()> {
        let mut futures: Vec<BoxFuture<'_, Result<Vec<u8>, TerminalError>>> = vec![];

        for req in requests {
            if req.one_way_call {
                ctx.send(
                    req.proxy_request.to_target(),
                    req.proxy_request.message,
                    req.proxy_request.delay_millis.map(Duration::from_millis),
                );
            } else {
                let fut = ctx
                    .call::<_, Vec<u8>>(req.proxy_request.to_target(), req.proxy_request.message);
                if req.await_at_the_end {
                    futures.push(fut.boxed())
                }
            }
        }

        for fut in futures {
            fut.await?;
        }

        Ok(())
    }
}
