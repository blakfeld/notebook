//! `GET /sync/v1/live`: the WebSocket live channel.
//!
//! The client authenticates with its first frame (browsers can't set headers
//! on a WebSocket, and URLs reach proxy logs). The server then subscribes the
//! connection to its vault's broadcast, reads the head, and sends `ready`;
//! anything committed after that read is on the channel, so `ready` followed
//! by an HTTP pull from the client's cursor misses nothing. Tokens are never
//! logged.

use std::net::IpAddr;
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use notebook_core::protocol::{
    CLOSE_AUTH, CLOSE_RESYNC, CLOSE_SHUTDOWN, CLOSE_TOO_MANY, ClientFrame, Entry, ErrorBody,
    PushOutcome, PushResponse, PushUpdate, ServerFrame,
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};

use crate::app::AppState;
use crate::auth::sessions::authenticate_token_at;
use crate::auth::{ApiError, AuthSession, is_active};
use crate::client_ip::ClientIp;
use crate::sync::SyncError;

/// Standard close code for a frame that isn't valid protocol JSON.
const CLOSE_POLICY: u16 = 1008;
/// Standard close code for binary frames.
const CLOSE_UNSUPPORTED: u16 = 1003;
/// Standard close code for a peer that stopped answering pings.
const CLOSE_GOING_AWAY: u16 = 1001;
/// Standard close code for a server-side failure.
const CLOSE_INTERNAL: u16 = 1011;

/// The origin (`scheme://host[:port]`) of `public_url`.
fn public_origin(public_url: &str) -> &str {
    let after_scheme = public_url.find("://").map_or(0, |i| i + 3);
    let end = public_url[after_scheme..]
        .find(['/', '?', '#'])
        .map_or(public_url.len(), |i| after_scheme + i);
    &public_url[..end]
}

pub async fn upgrade(
    State(state): State<AppState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Browsers always send `Origin`; native clients send none. A foreign
    // origin is a cross-site page trying to use the user's connection.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let expected = public_origin(&state.config.public_url);
        if !origin
            .to_str()
            .is_ok_and(|o| o.eq_ignore_ascii_case(expected))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let max = state.config.max_body_bytes.get();
    ws.max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| async move {
            Connection { state, ip, socket }.run().await;
        })
}

struct Connection {
    state: AppState,
    ip: IpAddr,
    socket: WebSocket,
}

/// Why the server ends a connection.
enum End {
    Close(u16, &'static str),
    /// The client closed or the transport failed; nothing to send.
    Gone,
}

impl Connection {
    async fn run(mut self) {
        let end = match self.authenticate().await {
            Ok(session) => self.serve(session).await,
            Err(end) => end,
        };
        if let End::Close(code, reason) = end {
            let frame = CloseFrame {
                code,
                reason: Utf8Bytes::from_static(reason),
            };
            let _ = self.write(Message::Close(Some(frame))).await;
        }
    }

    /// Writes one message. A client that stops reading fills the socket
    /// buffers; after a ping interval it is dropped, so a stalled send can't
    /// hold the connection (and the vault's broadcast buffer) open.
    async fn write(&mut self, message: Message) -> Result<(), End> {
        let wait = self.state.sync.settings.ping_interval;
        match tokio::time::timeout(wait, self.socket.send(message)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(End::Gone),
        }
    }

    async fn send(&mut self, frame: &ServerFrame) -> Result<(), End> {
        let text = serde_json::to_string(frame).map_err(|_| End::Gone)?;
        self.write(Message::text(text)).await
    }

    /// Waits for the first frame, which must be a valid `auth`.
    async fn authenticate(&mut self) -> Result<AuthSession, End> {
        let deadline = Instant::now() + self.state.sync.settings.auth_timeout;
        loop {
            let message = tokio::select! {
                message = self.socket.recv() => message,
                () = sleep_until(deadline) => {
                    return Err(End::Close(CLOSE_AUTH, "authentication timed out"));
                }
            };
            let text = match message {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Binary(_))) => {
                    return Err(End::Close(CLOSE_UNSUPPORTED, "binary frames unsupported"));
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(End::Gone),
            };
            match serde_json::from_str::<ClientFrame>(&text) {
                Ok(ClientFrame::Auth { token }) => {
                    return match check(&self.state, &token.0).await? {
                        Some(session) => Ok(session),
                        None => Err(End::Close(CLOSE_AUTH, "invalid token")),
                    };
                }
                Ok(ClientFrame::Unknown) => {}
                Ok(ClientFrame::Push { .. }) | Err(_) => {
                    return Err(End::Close(CLOSE_AUTH, "authenticate first"));
                }
            }
        }
    }

    /// The deadline for a token expiring at Unix second `expires_at`.
    fn expiry(&self, expires_at: i64) -> Instant {
        let left = u64::try_from(expires_at - self.state.auth.now()).unwrap_or(0);
        Instant::now() + Duration::from_secs(left)
    }

    async fn serve(&mut self, mut session: AuthSession) -> End {
        let sync = self.state.sync.clone();
        let Some((hub, _slot)) = sync.join(&session.vault_id) else {
            return End::Close(CLOSE_TOO_MANY, "too many connections");
        };
        let mut entries = hub.subscribe();
        let head = match super::store::head(&self.state.pool, &session.vault_id).await {
            Ok(head) => head,
            Err(_) => return End::Close(CLOSE_INTERNAL, "internal error"),
        };
        if let Err(end) = self.send(&ServerFrame::Ready { head }).await {
            return end;
        }

        let mut shutdown = sync.shutdown_signal();
        let mut expires = self.expiry(session.access_expires_at);
        let interval = sync.settings.ping_interval;
        let mut ping = tokio::time::interval_at(Instant::now() + interval, interval);
        let mut unanswered = 0u32;
        loop {
            tokio::select! {
                // Closing conditions first, so a busy channel can't delay them.
                biased;
                () = sleep_until(expires) => {
                    return End::Close(CLOSE_AUTH, "token expired");
                }
                _ = ping.tick() => {
                    if unanswered >= 2 {
                        return End::Close(CLOSE_GOING_AWAY, "no pong");
                    }
                    match is_active(&self.state.pool, &session.session_id).await {
                        Ok(true) => {}
                        Ok(false) => return End::Close(CLOSE_AUTH, "session ended"),
                        Err(e) => {
                            tracing::warn!("live session check failed: {e}");
                        }
                    }
                    if let Err(end) = self.write(Message::Ping(Default::default())).await {
                        return end;
                    }
                    unanswered += 1;
                }
                () = shutting_down(&mut shutdown) => {
                    return End::Close(CLOSE_SHUTDOWN, "server shutting down");
                }
                entry = entries.recv() => match entry {
                    Ok(entry) => {
                        let frame = ServerFrame::Entry(Entry::clone(&entry));
                        if let Err(end) = self.send(&frame).await {
                            return end;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        return End::Close(CLOSE_RESYNC, "fell behind; pull and reconnect");
                    }
                    Err(RecvError::Closed) => return End::Gone,
                },
                message = self.socket.recv() => {
                    let text = match message {
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(Message::Pong(_))) => {
                            unanswered = 0;
                            continue;
                        }
                        Some(Ok(Message::Ping(_))) => continue,
                        Some(Ok(Message::Binary(_))) => {
                            return End::Close(CLOSE_UNSUPPORTED, "binary frames unsupported");
                        }
                        Some(Ok(Message::Close(_)) | Err(_)) | None => return End::Gone,
                    };
                    match serde_json::from_str::<ClientFrame>(&text) {
                        Ok(ClientFrame::Auth { token }) => match check(&self.state, &token.0).await {
                            Ok(Some(next)) if next.vault_id == session.vault_id => {
                                expires = self.expiry(next.access_expires_at);
                                session = next;
                            }
                            Ok(_) => return End::Close(CLOSE_AUTH, "invalid token"),
                            Err(end) => return end,
                        },
                        Ok(ClientFrame::Push { request_id, updates }) => {
                            let outcome = push(&self.state, self.ip, &session, &updates).await;
                            let frame = ServerFrame::PushResult { request_id, outcome };
                            if let Err(end) = self.send(&frame).await {
                                return end;
                            }
                        }
                        Ok(ClientFrame::Unknown) => {}
                        Err(_) => return End::Close(CLOSE_POLICY, "unparseable frame"),
                    }
                }
            }
        }
    }
}

/// Checks a token; the session it names, or `None` if it's invalid.
async fn check(state: &AppState, token: &str) -> Result<Option<AuthSession>, End> {
    match authenticate_token_at(&state.pool, token, state.auth.now()).await {
        Ok(session) => Ok(Some(session)),
        Err(ApiError::INVALID_TOKEN) => Ok(None),
        Err(_) => Err(End::Close(CLOSE_INTERNAL, "internal error")),
    }
}

/// Resolves once the server starts shutting down.
async fn shutting_down(shutdown: &mut watch::Receiver<bool>) {
    let signalled = shutdown.wait_for(|&down| down).await.is_ok();
    if !signalled {
        std::future::pending::<()>().await;
    }
}

/// A socket push: the per-address limit, then the same store path as HTTP.
/// Rejections are results, not closes.
async fn push(
    state: &AppState,
    ip: IpAddr,
    session: &AuthSession,
    updates: &[PushUpdate],
) -> PushOutcome {
    let result = match state.limiter.check(ip) {
        Err(_) => Err(SyncError::RATE_LIMITED),
        Ok(()) => {
            state
                .sync
                .push(
                    &state.pool,
                    &session.vault_id,
                    updates,
                    state.config.sync_max_update_bytes.get(),
                )
                .await
        }
    };
    match result {
        Ok(pushed) => PushOutcome::Accepted(PushResponse {
            seqs: pushed.seqs,
            head: pushed.head,
        }),
        Err(SyncError(error)) => PushOutcome::Rejected(ErrorBody { error }),
    }
}

#[cfg(test)]
mod tests {
    use super::public_origin;

    #[test]
    fn origin_of_public_url() {
        assert_eq!(
            public_origin("https://notes.example.com"),
            "https://notes.example.com"
        );
        assert_eq!(
            public_origin("https://notes.example.com:8443/app?x#y"),
            "https://notes.example.com:8443"
        );
        assert_eq!(public_origin("http://localhost/"), "http://localhost");
    }
}
