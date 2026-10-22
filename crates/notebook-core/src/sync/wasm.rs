//! The browser transport (fetch, and the browser WebSocket through
//! gloo-net) and a `setTimeout` timer. The host spawns the engine with
//! `wasm_bindgen_futures::spawn_local`.

use std::cell::Cell;
use std::time::Duration;

use futures::future::{Either, select};
use futures::{SinkExt, StreamExt, pin_mut};
use gloo_net::http::{Request, RequestBuilder};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, WebSocketError};

use super::timer::Timer;
use super::transport::{
    LiveChannel, LiveEvent, Transport, TransportError, decode, live_event, parse_retry_after, urls,
};
use crate::auth::api::Token;
use crate::note::NoteId;
use crate::protocol::{ChangesResponse, ClientFrame, PushRequest, PushResponse, ServerFrame};

/// fetch has no timeout of its own.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// `/sync/v1` over fetch and the browser WebSocket.
#[derive(Debug, Clone)]
pub struct WasmTransport {
    http: String,
    live: String,
}

impl WasmTransport {
    /// `server` is the server's public URL, `https://host[:port]`.
    pub fn new(server: &str) -> Result<Self, TransportError> {
        let (http, live) = urls(server)?;
        Ok(Self { http, live })
    }

    /// Sends a request and returns the body of a `2xx` response.
    async fn send(
        &self,
        request: Result<Request, gloo_net::Error>,
    ) -> Result<Vec<u8>, TransportError> {
        let request = request.map_err(network)?;
        let send = request.send();
        let timeout = gloo_timers::future::sleep(REQUEST_TIMEOUT);
        pin_mut!(send, timeout);
        let response = match select(send, timeout).await {
            Either::Left((response, _)) => response.map_err(network)?,
            Either::Right(_) => return Err(TransportError::Network("request timed out".into())),
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .as_deref()
            .and_then(parse_retry_after);
        let body = response.binary().await.map_err(network)?;
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(TransportError::from_response(status, &body, retry_after))
        }
    }
}

fn authed(builder: RequestBuilder, token: &Token) -> RequestBuilder {
    builder.header("Authorization", &format!("Bearer {}", token.0))
}

impl Transport for WasmTransport {
    type Live = WasmLive;

    async fn push(
        &self,
        token: &Token,
        request: &PushRequest,
    ) -> Result<PushResponse, TransportError> {
        let body = serde_json::to_string(request).map_err(|_| TransportError::BadRequest)?;
        let request = authed(Request::post(&format!("{}/push", self.http)), token)
            .header("Content-Type", "application/json")
            .body(body);
        decode(&self.send(request).await?)
    }

    async fn changes(&self, token: &Token, after: u64) -> Result<ChangesResponse, TransportError> {
        let url = format!("{}/changes?after={after}", self.http);
        let request = authed(Request::get(&url), token).build();
        decode(&self.send(request).await?)
    }

    async fn upload_snapshot(
        &self,
        token: &Token,
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> Result<(), TransportError> {
        let url = format!("{}/notes/{note}/snapshot?covers={covers}", self.http);
        let request = authed(Request::put(&url), token)
            .header("Content-Type", "application/octet-stream")
            .body(js_sys::Uint8Array::from(data));
        self.send(request).await.map(drop)
    }

    async fn purge(&self, token: &Token, note: NoteId) -> Result<(), TransportError> {
        let url = format!("{}/notes/{note}", self.http);
        let request = authed(Request::delete(&url), token).build();
        self.send(request).await.map(drop)
    }

    async fn connect_live(&self) -> Result<WasmLive, TransportError> {
        // Opening completes in the background; the first send waits for it.
        let ws = WebSocket::open(&self.live).map_err(|e| TransportError::Network(e.to_string()))?;
        Ok(WasmLive { ws, ended: None })
    }
}

/// An open browser WebSocket. The browser answers pings itself.
pub struct WasmLive {
    ws: WebSocket,
    ended: Option<LiveEvent>,
}

impl LiveChannel for WasmLive {
    async fn next(&mut self) -> LiveEvent {
        if let Some(ended) = &self.ended {
            return ended.clone();
        }
        let event = loop {
            // The socket's messages come through a channel, so a dropped
            // `next` loses nothing.
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    // Frames of a type this build doesn't know are skipped.
                    if let Ok(frame) = serde_json::from_str::<ServerFrame>(&text) {
                        break live_event(frame);
                    }
                }
                Some(Ok(Message::Bytes(_))) => {}
                Some(Err(WebSocketError::ConnectionClose(close))) => {
                    break LiveEvent::Closed(Some(close.code));
                }
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
            .send(Message::Text(text))
            .await
            .map_err(|e| TransportError::Network(e.to_string()))
    }
}

/// The engine's clock in the browser: `Date.now()` relative to creation,
/// never going backwards (wall-clock adjustments are clamped).
#[derive(Debug)]
pub struct BrowserTimer {
    start: f64,
    last: Cell<u64>,
}

impl BrowserTimer {
    pub fn new() -> Self {
        Self {
            start: js_sys::Date::now(),
            last: Cell::new(0),
        }
    }
}

impl Default for BrowserTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for BrowserTimer {
    fn now(&self) -> u64 {
        let elapsed = (js_sys::Date::now() - self.start).max(0.0) as u64;
        let now = elapsed.max(self.last.get());
        self.last.set(now);
        now
    }

    async fn sleep(&self, duration: Duration) {
        gloo_timers::future::sleep(duration).await;
    }
}

fn network(e: gloo_net::Error) -> TransportError {
    TransportError::Network(e.to_string())
}
