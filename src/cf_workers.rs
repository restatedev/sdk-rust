
use std::str::FromStr;
use http::{HeaderName, HeaderValue, StatusCode};
use web_sys::{js_sys::Uint8Array, wasm_bindgen::{prelude::Closure, JsCast, JsValue}, ReadableStream, ReadableStreamDefaultController};
use worker::*;
use crate::prelude::Endpoint;

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

/// Http server to expose your Restate services.
pub struct CfWorkerServer {
    endpoint: Endpoint,
}

impl From<Endpoint> for CfWorkerServer {
    fn from(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl CfWorkerServer {
    /// Create new [`HttpServer`] from an [`Endpoint`].
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub fn call(&self, req: HttpRequest) -> worker::Result<http::Response<Body>> {
        let headers = req.headers().to_owned();
        let result = self.endpoint.resolve(req.uri().path(), headers);

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
                crate::endpoint::Response::BidiStream { status_code:_, headers:_, handler:_ } => {
                    // Cloudflare Workers don't support HTTP 1.1/HTTP 2 bididirectional streams
                    let http_response = http::Response::builder().status(StatusCode::NOT_IMPLEMENTED).body(worker::Body::empty())?;
                    // have to use worker::Result not http::Result
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
