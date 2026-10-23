//! [`AuthHttp`] over fetch, for the browser.

use std::time::Duration;

use futures::future::{Either, select};
use futures::pin_mut;
use gloo_net::http::Request;

use super::http::{AuthHttp, HttpRequest, HttpResponse, Method};
use crate::sync::parse_retry_after;
use crate::workspace::ServerUrl;

/// fetch has no timeout of its own.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct WasmAuthHttp {
    server: ServerUrl,
}

impl WasmAuthHttp {
    pub fn new(server: ServerUrl) -> Self {
        Self { server }
    }
}

impl AuthHttp for WasmAuthHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let url = format!("{}{}", self.server, request.path);
        let mut builder = match request.method {
            Method::Get => Request::get(&url),
            Method::Post => Request::post(&url),
            Method::Delete => Request::delete(&url),
        };
        if let Some(token) = &request.bearer {
            builder = builder.header("Authorization", &format!("Bearer {}", token.0));
        }
        let request = match request.body {
            Some(body) => builder
                .header("Content-Type", "application/json")
                .body(js_sys::Uint8Array::from(body.as_slice())),
            None => builder.build(),
        }
        .map_err(|e| e.to_string())?;
        let send = request.send();
        let timeout = gloo_timers::future::sleep(REQUEST_TIMEOUT);
        pin_mut!(send, timeout);
        let response = match select(send, timeout).await {
            Either::Left((response, _)) => response.map_err(|e| e.to_string())?,
            Either::Right(_) => return Err("request timed out".into()),
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .as_deref()
            .and_then(parse_retry_after);
        let body = response.binary().await.map_err(|e| e.to_string())?;
        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }
}
