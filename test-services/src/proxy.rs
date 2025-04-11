use futures::future::BoxFuture;
use futures::FutureExt;
use restate_sdk::context::RequestTarget;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyRequest {
    service_name: String,
    virtual_object_key: Option<String>,
    handler_name: String,
    idempotency_key: Option<String>,
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

#[derive(Serialize, Deserialize, JsonSchema)]
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
    async fn one_way_call(req: Json<ProxyRequest>) -> HandlerResult<String>;
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
        let mut request = ctx.request::<Vec<u8>, Vec<u8>>(req.to_target(), req.message);
        if let Some(idempotency_key) = req.idempotency_key {
            request = request.idempotency_key(idempotency_key);
        }
        Ok(request.call().await?.into())
    }

    async fn one_way_call(
        &self,
        ctx: Context<'_>,
        Json(req): Json<ProxyRequest>,
    ) -> HandlerResult<String> {
        let mut request = ctx.request::<_, ()>(req.to_target(), req.message);
        if let Some(idempotency_key) = req.idempotency_key {
            request = request.idempotency_key(idempotency_key);
        }

        let invocation_id = if let Some(delay_millis) = req.delay_millis {
            request
                .send_after(Duration::from_millis(delay_millis))
                .invocation_id()
                .await?
        } else {
            request.send().invocation_id().await?
        };

        Ok(invocation_id)
    }

    async fn many_calls(
        &self,
        ctx: Context<'_>,
        Json(requests): Json<Vec<ManyCallRequest>>,
    ) -> HandlerResult<()> {
        let mut futures: Vec<BoxFuture<'_, Result<Vec<u8>, TerminalError>>> = vec![];

        for req in requests {
            let mut restate_req =
                ctx.request::<_, Vec<u8>>(req.proxy_request.to_target(), req.proxy_request.message);
            if let Some(idempotency_key) = req.proxy_request.idempotency_key {
                restate_req = restate_req.idempotency_key(idempotency_key);
            }
            if req.one_way_call {
                if let Some(delay_millis) = req.proxy_request.delay_millis {
                    restate_req.send_after(Duration::from_millis(delay_millis));
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
