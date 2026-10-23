//! [`AuthHttp`] over reqwest (rustls), for native hosts.

use std::time::Duration;

use super::http::{AuthHttp, HttpRequest, HttpResponse, Method};
use crate::sync::parse_retry_after;
use crate::workspace::ServerUrl;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Auth responses are small; a key bundle is well under a kilobyte.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct NativeAuthHttp {
    client: reqwest::Client,
    server: ServerUrl,
}

impl NativeAuthHttp {
    pub fn new(server: ServerUrl) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // The API never redirects.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, server })
    }
}

impl AuthHttp for NativeAuthHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
        let url = format!("{}{}", self.server, request.path);
        let mut builder = match request.method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
            Method::Delete => self.client.delete(url),
        };
        if let Some(token) = &request.bearer {
            builder = builder.bearer_auth(&token.0);
        }
        if let Some(body) = request.body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        // reqwest errors name the URL, which holds no secrets.
        let mut response = builder.send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err("response too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }
}
