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
        Ok(ctx.request(req.to_target(), req.message).call().await?)
    }

    async fn one_way_call(
        &self,
        ctx: Context<'_>,
        Json(req): Json<ProxyRequest>,
    ) -> HandlerResult<()> {
        let request = ctx.request::<_, ()>(req.to_target(), req.message);

        if let Some(delay_millis) = req.delay_millis {
            request.send_with_delay(Duration::from_millis(delay_millis));
        } else {
            request.send();
        }

        Ok(())
    }

    async fn many_calls(
        &self,
        ctx: Context<'_>,
        Json(requests): Json<Vec<ManyCallRequest>>,
    ) -> HandlerResult<()> {
        let mut futures: Vec<BoxFuture<'_, Result<Vec<u8>, TerminalError>>> = vec![];

        for req in requests {
            let restate_req =
                ctx.request::<_, Vec<u8>>(req.proxy_request.to_target(), req.proxy_request.message);
            if req.one_way_call {
                if let Some(delay_millis) = req.proxy_request.delay_millis {
                    restate_req.send_with_delay(Duration::from_millis(delay_millis));
                } else {
                    restate_req.send();
                }
            } else {
                let fut = restate_req.call();
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
