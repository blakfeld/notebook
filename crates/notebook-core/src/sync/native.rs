//! The native transport (reqwest over rustls, tokio-tungstenite) and a tokio
//! timer. The engine runs on a current-thread tokio runtime.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::timer::Timer;
use super::transport::{
    LiveChannel, LiveEvent, Transport, TransportError, decode, live_event, parse_retry_after, urls,
};
use crate::auth::api::Token;
use crate::note::NoteId;
use crate::protocol::{ChangesResponse, ClientFrame, PushRequest, PushResponse, ServerFrame};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// `/sync/v1` over HTTPS (or HTTP for a local server) and its WebSocket.
#[derive(Debug, Clone)]
pub struct NativeTransport {
    client: reqwest::Client,
    /// `https://host[:port]/sync/v1`
    http: String,
    /// `wss://host[:port]/sync/v1/live`
    live: String,
}

impl NativeTransport {
    /// `server` is the server's public URL, `https://host[:port]` (a path
    /// or trailing slash is ignored).
    pub fn new(server: &str) -> Result<Self, TransportError> {
        let (http, live) = urls(server)?;
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // The API never redirects.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| TransportError::Network(e.to_string()))?;
        Ok(Self { client, http, live })
    }

    /// Sends a request and returns the body of a `2xx` response.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Vec<u8>, TransportError> {
        let mut response = request.send().await.map_err(network)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        // Pages are about 2 MiB; refuse to buffer a runaway body.
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(TransportError::Network("response too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(TransportError::from_response(status, &body, retry_after))
        }
    }
}

impl Transport for NativeTransport {
    type Live = NativeLive;

    async fn push(
        &self,
        token: &Token,
        request: &PushRequest,
    ) -> Result<PushResponse, TransportError> {
        let body = serde_json::to_vec(request).map_err(|_| TransportError::BadRequest)?;
        let request = self
            .client
            .post(format!("{}/push", self.http))
            .bearer_auth(&token.0)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        decode(&self.send(request).await?)
    }

    async fn changes(&self, token: &Token, after: u64) -> Result<ChangesResponse, TransportError> {
        let request = self
            .client
            .get(format!("{}/changes?after={after}", self.http))
            .bearer_auth(&token.0);
        decode(&self.send(request).await?)
    }

    async fn upload_snapshot(
        &self,
        token: &Token,
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> Result<(), TransportError> {
        let request = self
            .client
            .put(format!(
                "{}/notes/{note}/snapshot?covers={covers}",
                self.http
            ))
            .bearer_auth(&token.0)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(data.to_vec());
        self.send(request).await.map(drop)
    }

    async fn purge(&self, token: &Token, note: NoteId) -> Result<(), TransportError> {
        let request = self
            .client
            .delete(format!("{}/notes/{note}", self.http))
            .bearer_auth(&token.0);
        self.send(request).await.map(drop)
    }

    async fn connect_live(&self) -> Result<NativeLive, TransportError> {
        let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(self.live.as_str()))
            .await
            .map_err(|_| TransportError::Network("live channel connect timed out".into()))?
            .map_err(|e| TransportError::Network(e.to_string()))?;
        Ok(NativeLive { ws, ended: None })
    }
}

/// An open live channel. Pings are answered by tungstenite while reading.
pub struct NativeLive {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ended: Option<LiveEvent>,
}

impl LiveChannel for NativeLive {
    async fn next(&mut self) -> LiveEvent {
        if let Some(ended) = &self.ended {
            return ended.clone();
        }
        let event = loop {
            // `StreamExt::next` is cancel-safe: a dropped call loses nothing.
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    // Frames of a type this build doesn't know are skipped.
                    if let Ok(frame) = serde_json::from_str::<ServerFrame>(&text) {
                        break live_event(frame);
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    break LiveEvent::Closed(frame.map(|f| f.code.into()));
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => break LiveEvent::Error(TransportError::Network(e.to_string())),
                None => break LiveEvent::Closed(None),
            }
        };
        if matches!(event, LiveEvent::Closed(_) | LiveEvent::Error(_)) {
            self.ended = Some(event.clone());
        }
        event
    }

    async fn send(&mut self, frame: &ClientFrame) -> Result<(), TransportError> {
        let text = serde_json::to_string(frame).map_err(|_| TransportError::BadRequest)?;
        self.ws
            .send(Message::text(text))
            .await
            .map_err(|e| TransportError::Network(e.to_string()))
    }
}

/// The engine's clock: milliseconds since the timer was created.
#[derive(Debug, Clone, Copy)]
pub struct TokioTimer {
    start: tokio::time::Instant,
}

impl TokioTimer {
    pub fn new() -> Self {
        Self {
            start: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for TokioTimer {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

fn network(e: reqwest::Error) -> TransportError {
    // reqwest errors name the URL, which holds no secrets (tokens go in a
    // header).
    TransportError::Network(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ErrorBody, ErrorCode, PushOutcome};

    #[test]
    fn builds_urls_from_the_public_url() {
        assert_eq!(
            urls("https://notes.example.com").unwrap(),
            (
                "https://notes.example.com/sync/v1".into(),
                "wss://notes.example.com/sync/v1/live".into()
            )
        );
        assert_eq!(
            urls("http://127.0.0.1:8080/app/").unwrap(),
            (
                "http://127.0.0.1:8080/sync/v1".into(),
                "ws://127.0.0.1:8080/sync/v1/live".into()
            )
        );
        assert!(urls("http://localhost:3000").is_ok());
        assert!(urls("http://[::1]:3000").is_ok());
        for bad in [
            "notes.example.com",
            "ftp://x",
            "https://",
            "https:///x",
            // Cleartext only to this machine.
            "http://notes.example.com",
            "http://192.168.1.10:8080",
        ] {
            assert!(urls(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after(" 5 "), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[test]
    fn maps_rejected_live_pushes_like_http_errors() {
        let frame = ServerFrame::PushResult {
            request_id: "r".into(),
            outcome: PushOutcome::Rejected(ErrorBody {
                error: ErrorCode::NotePurged,
            }),
        };
        assert_eq!(
            live_event(frame),
            LiveEvent::PushResult {
                request_id: "r".into(),
                result: Err(TransportError::NotePurged),
            }
        );
        let frame = ServerFrame::PushResult {
            request_id: "r".into(),
            outcome: PushOutcome::Rejected(ErrorBody {
                error: ErrorCode::Internal,
            }),
        };
        assert!(matches!(
            live_event(frame),
            LiveEvent::PushResult {
                result: Err(TransportError::Server(500)),
                ..
            }
        ));
    }

    #[test]
    fn maps_error_responses() {
        let body = br#"{"error":"cursor_ahead"}"#;
        assert_eq!(
            TransportError::from_response(409, body, None),
            TransportError::CursorAhead
        );
        assert_eq!(
            TransportError::from_response(429, b"", parse_retry_after("12")),
            TransportError::RateLimited {
                retry_after: Some(Duration::from_secs(12))
            }
        );
    }
}
