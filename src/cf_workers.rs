
use std::{ops::Deref, str::FromStr};
use http::{response, HeaderName, HeaderValue, StatusCode};
use restate_sdk_shared_core::Header;
use tokio::sync::mpsc;
use web_sys::{js_sys::Uint8Array, wasm_bindgen::{prelude::Closure, JsCast, JsValue},
    ReadableStream, ReadableStreamDefaultController, ReadableStreamDefaultReader};
use wasm_bindgen_futures;
use worker::*;
use crate::{endpoint::{InputReceiver, OutputSender}, prelude::Endpoint};

#[allow(clippy::declare_interior_mutable_const)]
const X_RESTATE_SERVER: HeaderName = HeaderName::from_static("x-restate-server");
const X_RESTATE_SERVER_VALUE: HeaderValue =
    HeaderValue::from_static(concat!("restate-sdk-rust/", env!("CARGO_PKG_VERSION")));

// Convert Bytes to ReadableStream using Web API bindings
fn bytes_to_readable_stream(data: bytes::Bytes) ->  core::result::Result<ReadableStream, JsValue> {
    let underlying_source = js_sys::Object::new();

    let start_closure = Closure::wrap(Box::new(move |controller: ReadableStreamDefaultController| {
        // Convert bytes to Uint8Array
        let uint8_array = Uint8Array::new_with_length(data.len() as u32);
        uint8_array.copy_from(&data[..]);

        // Enqueue the data
        let _ = controller.enqueue_with_chunk(&uint8_array);
        let _ = controller.close();

    }) as Box<dyn FnMut(ReadableStreamDefaultController)>);

    js_sys::Reflect::set(
        &underlying_source,
        &JsValue::from_str("start"),
        start_closure.as_ref().unchecked_ref(),
    )?;

    start_closure.forget(); // Prevent cleanup

    ReadableStream::new_with_underlying_source(&underlying_source)
}

/// Cloudflare Worker server to expose Restate services.
pub struct CfWorkerServer {
    endpoint: Endpoint,
}

impl From<Endpoint> for CfWorkerServer {
    fn from(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl CfWorkerServer {

    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub async fn call(&self, req: HttpRequest) -> worker::Result<http::Response<worker::Body>> {
        let headers = req.headers().to_owned();
        let (parts, body) = req.into_parts();
        let result = self.endpoint.resolve(parts.uri.path(), headers);

        if let Ok(response) = result {
            match response {
                crate::endpoint::Response::ReplyNow { status_code, headers, body } => {

                    let readable_stream = bytes_to_readable_stream(body)?;
                    let mut http_response = http::Response::builder()
                        .status(status_code)
                        .body(Body::new(readable_stream))?;

                    for header in headers {
                        let key = HeaderName::from_str(header.key.as_ref())?;
                        let value = HeaderValue::from_str(header.value.as_ref())?;
                        http_response.headers_mut().insert(key, value);
                    }

                    Ok(http_response)
                }
                crate::endpoint::Response::BidiStream { status_code, headers, handler} => {

                    // Cloudflare Workers don't support HTTP 1.1/HTTP 2 bididirectional streams
                    // Implenting this as a workaround using existing handler api, expecting a hyper request stream
                    // Reads entire request/reponse body to proxy data across WASM boundary

                    let js_stream: ReadableStream = body.into_inner().unwrap().unchecked_into();
                    let reader: ReadableStreamDefaultReader = js_stream.get_reader().unchecked_into();

                    let mut request_body = Vec::new();
                    loop {
                        let read_result = wasm_bindgen_futures::JsFuture::from(reader.read()).await;
                        match read_result {
                            Ok(js_value) => {
                                let done = js_sys::Reflect::get(&js_value, &JsValue::from_str("done")).unwrap();
                                if done.as_bool().unwrap_or(false) {
                                    break;
                                }
                                let value = js_sys::Reflect::get(&js_value, &JsValue::from_str("value")).unwrap();
                                let uint8_array: Uint8Array = value.unchecked_into();
                                let mut bytes = vec![0u8; uint8_array.length() as usize];
                                uint8_array.copy_to(&mut bytes);
                                request_body.extend_from_slice(&bytes);
                            }
                            Err(_) => break,
                        }
                    }

                    // Create a simple stream from the collected bytes -- this maps to existing restate sdk stream implementation for hanlders
                    let request_bytes = bytes::Bytes::from(request_body);
                    let stream = futures_util::stream::once(async move {
                        Ok::<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>(request_bytes)
                    });

                    let input_receiver = InputReceiver::from_stream(stream);

                    let (output_tx, output_rx) = mpsc::unbounded_channel();
                    let output_sender = OutputSender::from_channel(output_tx);

                    // Execute handler and collect output
                    let _ = handler.handle(input_receiver, output_sender).await;

                    // Collect all output chunks to proxy body response across WASM boundary
                    let mut response_body = Vec::new();
                    let mut rx = output_rx;
                    while let Some(chunk) = rx.recv().await {
                        response_body.extend_from_slice(&chunk);
                    }

                    // Build HTTP Response from bytes
                    let readable_stream = bytes_to_readable_stream(bytes::Bytes::from(response_body))?;
                    let mut http_response = http::Response::builder()
                        .status(status_code)
                        .body(Body::new(readable_stream))?;

                    for header in headers {
                        let key = HeaderName::from_str(header.key.as_ref())?;
                        let value = HeaderValue::from_str(header.value.as_ref())?;
                        http_response.headers_mut().insert(key, value);
                    }

                    Ok(http_response)
                },
            }
        }
        else {
            let http_response = http::Response::builder().status(StatusCode::BAD_REQUEST).body(worker::Body::empty())?;
            Ok(http_response)
        }
    }
}
