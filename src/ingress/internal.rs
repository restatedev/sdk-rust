use std::time::Duration;

use reqwest::{header::HeaderMap, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::{
    request::{IngressRequestOptions, SendResponse, SendStatus},
    result::{IngressResultOptions, ResultOp, ResultTarget},
};
use crate::{context::RequestTarget, errors::TerminalError};

const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendResponseSchema {
    invocation_id: String,
    status: SendStatusSchema,
}

#[derive(Deserialize)]
enum SendStatusSchema {
    Accepted,
    PreviouslyAccepted,
}

impl From<SendStatusSchema> for SendStatus {
    fn from(value: SendStatusSchema) -> Self {
        match value {
            SendStatusSchema::Accepted => SendStatus::Accepted,
            SendStatusSchema::PreviouslyAccepted => SendStatus::PreviouslyAccepted,
        }
    }
}

#[derive(Deserialize)]
struct TerminalErrorSchema {
    code: Option<u16>,
    message: String,
}

pub(super) struct IngressInternal {
    pub(super) client: reqwest::Client,
    pub(super) url: Url,
    pub(super) headers: HeaderMap,
}

impl IngressInternal {
    pub(super) async fn call<Req: Serialize, Res: DeserializeOwned>(
        &self,
        target: RequestTarget,
        req: Req,
        opts: IngressRequestOptions,
    ) -> Result<Result<Res, TerminalError>, reqwest::Error> {
        let mut headers = self.headers.clone();
        if let Some(key) = opts.idempotency_key {
            headers.append(IDEMPOTENCY_KEY_HEADER, key);
        }

        let url = format!("{}/{target}", self.url.as_str().trim_end_matches("/"));

        let mut builder = self.client.post(url).headers(headers).json(&req);

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Ok(Err(TerminalError::new_with_code(
                    e.code.unwrap_or(status),
                    e.message,
                )))
            } else {
                Err(e)
            }
        } else {
            Ok(Ok(res.json::<Res>().await?))
        }
    }

    pub(super) async fn send<Req: Serialize>(
        &self,
        target: RequestTarget,
        req: Req,
        opts: IngressRequestOptions,
        delay: Option<Duration>,
    ) -> Result<Result<SendResponse, TerminalError>, reqwest::Error> {
        let mut headers = self.headers.clone();
        let attachable = if let Some(key) = opts.idempotency_key {
            headers.append(IDEMPOTENCY_KEY_HEADER, key);
            true
        } else {
            false
        };

        let url = if let Some(delay) = delay {
            format!(
                "{}/{target}/send?delay={}ms",
                self.url.as_str().trim_end_matches("/"),
                delay.as_millis()
            )
        } else {
            format!("{}/{target}/send", self.url.as_str().trim_end_matches("/"))
        };

        let mut builder = self.client.post(url).headers(headers).json(&req);

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Ok(Err(TerminalError::new_with_code(
                    e.code.unwrap_or(status),
                    e.message,
                )))
            } else {
                Err(e)
            }
        } else {
            let res = res.json::<SendResponseSchema>().await?;
            Ok(Ok(SendResponse {
                invocation_id: res.invocation_id,
                status: res.status.into(),
                attachable,
            }))
        }
    }

    pub(super) async fn result<Res: DeserializeOwned>(
        &self,
        target: ResultTarget,
        op: ResultOp,
        opts: IngressResultOptions,
    ) -> Result<Result<Res, TerminalError>, reqwest::Error> {
        let url = format!("{}/{target}/{op}", self.url.as_str().trim_end_matches("/"));

        let mut builder = self.client.get(url).headers(self.headers.clone());

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Ok(Err(TerminalError::new_with_code(
                    e.code.unwrap_or(status),
                    e.message,
                )))
            } else {
                Err(e)
            }
        } else {
            Ok(Ok(res.json::<Res>().await?))
        }
    }
}
