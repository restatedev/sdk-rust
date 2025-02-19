use std::time::Duration;

use http::HeaderValue;
use reqwest::{header::HeaderMap, Url};
use thiserror::Error;

use super::{
    awakeable::IngressAwakeableOptions,
    handle::{HandleOp, HandleTarget, IngressHandleOptions},
    request::{IngressRequestOptions, SendResponse, SendStatus},
};
use crate::{
    context::RequestTarget,
    errors::TerminalError,
    serde::{Deserialize, Serialize},
};

const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";
const APPLICATION_JSON: HeaderValue = HeaderValue::from_static("application/json");
const TEXT_PLAIN: HeaderValue = HeaderValue::from_static("text/plain");

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendResponseSchema {
    invocation_id: String,
    status: SendStatusSchema,
}

#[derive(serde::Deserialize)]
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

#[derive(serde::Deserialize)]
struct TerminalErrorSchema {
    code: Option<u16>,
    message: String,
}

pub(super) struct IngressInternal {
    pub(super) client: reqwest::Client,
    pub(super) url: Url,
    pub(super) headers: HeaderMap,
}

#[derive(Debug, Error)]
pub enum IngressClientError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("terminal error [{}]: {}", ._0.code(), ._0.message())]
    Terminal(TerminalError),
    #[error(transparent)]
    Serde(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl From<TerminalError> for IngressClientError {
    fn from(value: TerminalError) -> Self {
        Self::Terminal(value)
    }
}

impl IngressInternal {
    pub(super) async fn call<Req: Serialize, Res: Deserialize>(
        &self,
        target: RequestTarget,
        req: Req,
        opts: IngressRequestOptions,
    ) -> Result<Res, IngressClientError> {
        let mut headers = self.headers.clone();
        if let Some(key) = opts.idempotency_key {
            headers.append(IDEMPOTENCY_KEY_HEADER, key);
        }

        let url = format!("{}/{target}", self.url.as_str().trim_end_matches("/"));

        let mut builder = self.client.post(url).headers(headers).body(
            req.serialize()
                .map_err(|e| IngressClientError::Serde(Box::new(e)))?,
        );

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Err(TerminalError::new_with_code(e.code.unwrap_or(status), e.message).into())
            } else {
                Err(e.into())
            }
        } else {
            Ok(Res::deserialize(&mut res.bytes().await?)
                .map_err(|e| IngressClientError::Serde(Box::new(e)))?)
        }
    }

    pub(super) async fn send<Req: Serialize>(
        &self,
        target: RequestTarget,
        req: Req,
        opts: IngressRequestOptions,
        delay: Option<Duration>,
    ) -> Result<SendResponse, IngressClientError> {
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

        let mut builder = self.client.post(url).headers(headers).body(
            req.serialize()
                .map_err(|e| IngressClientError::Serde(Box::new(e)))?,
        );

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Err(TerminalError::new_with_code(e.code.unwrap_or(status), e.message).into())
            } else {
                Err(e.into())
            }
        } else {
            let res = res.json::<SendResponseSchema>().await?;
            Ok(SendResponse {
                invocation_id: res.invocation_id,
                status: res.status.into(),
                attachable,
            })
        }
    }

    pub(super) async fn handle<Res: Deserialize>(
        &self,
        target: HandleTarget,
        op: HandleOp,
        opts: IngressHandleOptions,
    ) -> Result<Res, IngressClientError> {
        let url = format!("{}/{target}/{op}", self.url.as_str().trim_end_matches("/"));

        let mut builder = self.client.get(url).headers(self.headers.clone());

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Err(TerminalError::new_with_code(e.code.unwrap_or(status), e.message).into())
            } else {
                Err(e.into())
            }
        } else {
            Ok(Res::deserialize(&mut res.bytes().await?)
                .map_err(|e| IngressClientError::Serde(Box::new(e)))?)
        }
    }

    pub(super) async fn resolve_awakeable<T: Serialize + 'static>(
        &self,
        key: &str,
        payload: Option<T>,
        opts: IngressAwakeableOptions,
    ) -> Result<(), IngressClientError> {
        let url = format!(
            "{}/restate/awakeables/{}/resolve",
            self.url.as_str().trim_end_matches("/"),
            key
        );

        let mut builder = self.client.post(url).headers(self.headers.clone());

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        if let Some(payload) = payload {
            builder = builder
                .header(http::header::CONTENT_TYPE, APPLICATION_JSON)
                .body(
                    payload
                        .serialize()
                        .map_err(|e| IngressClientError::Serde(Box::new(e)))?,
                );
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Err(TerminalError::new_with_code(e.code.unwrap_or(status), e.message).into())
            } else {
                Err(e.into())
            }
        } else {
            Ok(())
        }
    }

    pub(super) async fn reject_awakeable(
        &self,
        key: &str,
        message: &str,
        opts: IngressAwakeableOptions,
    ) -> Result<(), IngressClientError> {
        let url = format!(
            "{}/restate/awakeables/{}/reject",
            self.url.as_str().trim_end_matches("/"),
            key
        );

        let mut builder = self
            .client
            .post(url)
            .headers(self.headers.clone())
            .header(http::header::CONTENT_TYPE, TEXT_PLAIN)
            .body(message.to_string());

        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }

        let res = builder.send().await?;

        if let Err(e) = res.error_for_status_ref() {
            let status = res.status().as_u16();
            if let Ok(e) = res.json::<TerminalErrorSchema>().await {
                Err(TerminalError::new_with_code(e.code.unwrap_or(status), e.message).into())
            } else {
                Err(e.into())
            }
        } else {
            Ok(())
        }
    }
}
