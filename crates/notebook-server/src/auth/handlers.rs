//! `/auth/*` handlers. Failures are logged at `debug` with a reason category
//! and never the username, so logs aren't an account-existence oracle.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use notebook_core::auth::api::{
    KeyBundleResponse, LoginFinishRequest, LoginFinishResponse, LoginStartRequest,
    LoginStartResponse, PassphraseFinishRequest, PassphraseStartRequest, PassphraseStartResponse,
    RecoverFinishRequest, RecoverStartRequest, RecoverStartResponse, RefreshRequest,
    RegisterFinishRequest, RegisterStartRequest, RegisterStartResponse, Session, SessionInfo,
    Tokens,
};
use notebook_core::auth::{SecureRng, Suite, Username};
use notebook_core::crypto::{KeyBundle, RECOVERY_VERIFIER_LEN};
use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration,
};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::error::{ApiError, ApiJson};
use super::sessions::{self, AuthSession, Issued};
use super::store::{self, CreateError};
use super::{MAX_DEVICE_NAME_CHARS, Pending, ct_eq};
use crate::app::AppState;
use crate::invites;
use crate::rate_limit::too_many_requests;

type Result<T, E = ApiError> = std::result::Result<T, E>;

fn fail(reason: &str, error: ApiError) -> ApiError {
    tracing::debug!(reason, "auth request failed");
    error
}

fn username(text: &str) -> Result<Username> {
    Username::parse(text).map_err(|_| fail("invalid username", ApiError::INVALID_USERNAME))
}

fn check_device_name(name: &str) -> Result<()> {
    if name.chars().count() > MAX_DEVICE_NAME_CHARS {
        return Err(fail("device name too long", ApiError::INVALID_REQUEST));
    }
    Ok(())
}

fn bad_request<E>(reason: &'static str) -> impl FnOnce(E) -> ApiError {
    move |_| fail(reason, ApiError::INVALID_REQUEST)
}

/// Checks the per-username limit, returning `Some(429)` when over it.
/// Over-limit requests do no OPAQUE work.
fn limit_username(state: &AppState, username: &Username) -> Option<Response> {
    state
        .auth
        .username_limiter
        .check(username.as_str())
        .err()
        .map(|wait| {
            tracing::debug!("per-username rate limit hit");
            too_many_requests(wait)
        })
}

fn registration_response(state: &AppState, request: &[u8], username: &Username) -> Result<Vec<u8>> {
    let request = RegistrationRequest::<Suite>::deserialize(request)
        .map_err(bad_request("registration request"))?;
    let start = ServerRegistration::start(&state.auth.setup, request, username.as_str().as_bytes())
        .map_err(bad_request("registration start"))?;
    Ok(start.message.serialize().to_vec())
}

/// The record to store from a client's registration upload.
fn opaque_record(upload: &[u8]) -> Result<Vec<u8>> {
    let upload = RegistrationUpload::<Suite>::deserialize(upload)
        .map_err(bad_request("registration upload"))?;
    Ok(ServerRegistration::finish(upload).serialize().to_vec())
}

fn session_body(issued: Issued, username: &str, is_admin: bool) -> Session {
    Session {
        session_id: sessions::session_id_text(&issued.session_id),
        access_token: issued.access_token,
        access_expires_in: sessions::ACCESS_TTL_SECS as u64,
        refresh_token: issued.refresh_token,
        username: username.to_owned(),
        is_admin,
    }
}

fn pending_full<T>(_: T) -> ApiError {
    tracing::warn!("too many pending auth handshakes");
    ApiError::UNAVAILABLE
}

// --- registration ---

fn invalid_invite() -> ApiError {
    fail("invalid invite", ApiError::INVALID_INVITE)
}

/// Checked first by both registration steps, before the username is parsed
/// or any OPAQUE work is done, so a caller without a valid code learns
/// nothing about usernames. Every invalid code fails the same way.
async fn check_invite(state: &AppState, code: &str) -> Result<()> {
    let mut conn = state.pool.acquire().await?;
    if invites::check(&mut conn, code, state.auth.now()).await? {
        Ok(())
    } else {
        Err(invalid_invite())
    }
}

pub async fn register_start(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RegisterStartRequest>,
) -> Result<Json<RegisterStartResponse>> {
    check_invite(&state, &req.invite_code).await?;
    let username = username(&req.username)?;
    Ok(Json(RegisterStartResponse {
        registration_response: registration_response(&state, &req.registration_request, &username)?,
    }))
}

pub async fn register_finish(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RegisterFinishRequest>,
) -> Result<Json<Session>> {
    check_invite(&state, &req.invite_code).await?;
    let username = username(&req.username)?;
    check_device_name(&req.device_name)?;
    let record = opaque_record(&req.registration_upload)?;
    let bundle = KeyBundle::from_bytes(&req.key_bundle).map_err(bad_request("key bundle"))?;
    let verifier: [u8; RECOVERY_VERIFIER_LEN] = req
        .recovery_verifier
        .as_slice()
        .try_into()
        .map_err(bad_request("recovery verifier"))?;

    let now = state.auth.now();
    let user_id = store::new_user_id(now);
    // The write lock from `BEGIN IMMEDIATE` serializes concurrent finishes,
    // so only one can consume the code; if creating the account fails
    // below, the rollback leaves the invite redeemable.
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    if !invites::consume(&mut tx, &req.invite_code, &user_id, now).await? {
        return Err(invalid_invite());
    }
    let account = store::NewAccount {
        user_id,
        username: username.as_str(),
        opaque_record: &record,
        recovery_verifier_hash: Sha256::digest(verifier).into(),
        is_admin: false,
        vault_id: bundle.vault_id().to_bytes(),
        key_bundle: &req.key_bundle,
        now,
    };
    match store::create_account(&mut tx, &account, state.config.default_quota()).await {
        Ok(()) => {}
        Err(CreateError::UsernameTaken) => {
            return Err(fail("username taken", ApiError::USERNAME_TAKEN));
        }
        Err(CreateError::VaultTaken) => {
            return Err(fail("vault id in use", ApiError::INVALID_REQUEST));
        }
        Err(CreateError::Db(e)) => return Err(e.into()),
    }
    let issued = sessions::issue(
        &mut tx,
        &user_id,
        &account.vault_id,
        &req.device_name,
        req.session_kind,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(session_body(issued, username.as_str(), false)))
}

// --- login ---

/// Runs the server side of an OPAQUE login. With no record (unknown user)
/// `opaque-ke` answers with a fake response that is indistinguishable from a
/// real one and derived from the OPRF seed and username, so it is stable.
fn server_login(
    state: &AppState,
    user: Option<&store::User>,
    credential_request: &[u8],
    username: &str,
) -> Result<(Box<ServerLogin<Suite>>, Vec<u8>)> {
    let request = CredentialRequest::<Suite>::deserialize(credential_request)
        .map_err(bad_request("credential request"))?;
    let record = match user {
        Some(user) => Some(
            ServerRegistration::<Suite>::deserialize(&user.opaque_record).map_err(|_| {
                tracing::error!("stored OPAQUE record does not parse");
                ApiError::INTERNAL
            })?,
        ),
        None => None,
    };
    let start = ServerLogin::start(
        &mut SecureRng,
        &state.auth.setup,
        record,
        request,
        username.as_bytes(),
        ServerLoginParameters::default(),
    )
    .map_err(bad_request("login start"))?;
    Ok((Box::new(start.state), start.message.serialize().to_vec()))
}

/// Fails with `invalid_credentials` unless the user's OPAQUE record is still
/// `expected`, so a handshake started before a passphrase change or recovery
/// can't finish after it and outlive the revocation that came with it.
async fn check_record_unchanged(
    conn: &mut SqliteConnection,
    user_id: &[u8],
    expected: &[u8],
) -> Result<()> {
    let current = store::find_by_id(conn, user_id).await?;
    match current {
        Some(user) if ct_eq(&user.opaque_record, expected) => Ok(()),
        _ => Err(fail(
            "credentials changed during handshake",
            ApiError::INVALID_CREDENTIALS,
        )),
    }
}

fn finish_login(state: ServerLogin<Suite>, finalization: &[u8]) -> Result<()> {
    let finalization = CredentialFinalization::<Suite>::deserialize(finalization)
        .map_err(|_| fail("credential finalization", ApiError::INVALID_CREDENTIALS))?;
    state
        .finish(finalization, ServerLoginParameters::default())
        .map_err(|_| fail("wrong passphrase", ApiError::INVALID_CREDENTIALS))?;
    Ok(())
}

pub async fn login_start(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<LoginStartRequest>,
) -> Result<Response> {
    let username = username(&req.username)?;
    if let Some(response) = limit_username(&state, &username) {
        return Ok(response);
    }
    let mut conn = state.pool.acquire().await?;
    let user = store::find_by_username(&mut conn, username.as_str()).await?;
    drop(conn);
    let (login, credential_response) = server_login(
        &state,
        user.as_ref(),
        &req.credential_request,
        username.as_str(),
    )?;
    let handshake_id = state
        .auth
        .pending
        .insert(Pending::Login { state: login, user })
        .map_err(pending_full)?;
    Ok(Json(LoginStartResponse {
        handshake_id,
        credential_response,
    })
    .into_response())
}

pub async fn login_finish(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<LoginFinishRequest>,
) -> Result<Json<LoginFinishResponse>> {
    check_device_name(&req.device_name)?;
    let Some(Pending::Login { state: login, user }) = state.auth.pending.take(&req.handshake_id)
    else {
        return Err(fail(
            "unknown or expired handshake",
            ApiError::INVALID_CREDENTIALS,
        ));
    };
    finish_login(*login, &req.credential_finalization)?;
    // A fake record can't complete the key exchange, so an unknown user has
    // already failed above; this is defense in depth.
    let user = user.ok_or_else(|| fail("unknown user", ApiError::INVALID_CREDENTIALS))?;

    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    check_record_unchanged(&mut tx, &user.user_id, &user.opaque_record).await?;
    let key_bundle = store::key_bundle(&mut tx, &user.vault_id)
        .await?
        .ok_or(ApiError::INTERNAL)?;
    let issued = sessions::issue(
        &mut tx,
        &user.user_id,
        &user.vault_id,
        &req.device_name,
        req.session_kind,
        state.auth.now(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(LoginFinishResponse {
        session: session_body(issued, &user.username, user.is_admin),
        key_bundle,
    }))
}

// --- sessions ---

pub async fn refresh(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RefreshRequest>,
) -> Result<Json<Tokens>> {
    sessions::refresh(&state.pool, &req.refresh_token.0, state.auth.now())
        .await
        .map(Json)
}

pub async fn key_bundle(
    State(state): State<AppState>,
    session: AuthSession,
) -> Result<Json<KeyBundleResponse>> {
    let mut conn = state.pool.acquire().await?;
    let key_bundle = store::key_bundle(&mut conn, &session.vault_id)
        .await?
        .ok_or(ApiError::INTERNAL)?;
    Ok(Json(KeyBundleResponse { key_bundle }))
}

pub async fn logout(State(state): State<AppState>, session: AuthSession) -> Result<StatusCode> {
    sessions::revoke(&state.pool, &session.user_id, &session.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(
    State(state): State<AppState>,
    session: AuthSession,
) -> Result<Json<Vec<SessionInfo>>> {
    Ok(Json(
        sessions::list(&state.pool, &session.user_id, &session.session_id).await?,
    ))
}

/// `404` for an unknown ID or another user's session, which is left alone.
pub async fn revoke_session(
    State(state): State<AppState>,
    session: AuthSession,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let Some(id) = sessions::parse_session_id(&id) else {
        return Err(ApiError::NOT_FOUND);
    };
    if sessions::revoke(&state.pool, &session.user_id, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NOT_FOUND)
    }
}

// --- passphrase change and recovery ---

/// Checks a replacement bundle against the stored one: it must parse, name
/// the same vault, and carry a byte-identical recovery entry.
async fn check_replacement_bundle(
    conn: &mut SqliteConnection,
    vault_id: &[u8],
    replacement: &[u8],
) -> Result<()> {
    let new = KeyBundle::from_bytes(replacement).map_err(bad_request("key bundle"))?;
    let stored = store::key_bundle(conn, vault_id)
        .await?
        .ok_or(ApiError::INTERNAL)?;
    let stored = KeyBundle::from_bytes(&stored).map_err(|_| {
        tracing::error!("stored key bundle does not parse");
        ApiError::INTERNAL
    })?;
    if new.vault_id().to_bytes() != vault_id {
        return Err(fail(
            "bundle names another vault",
            ApiError::INVALID_REQUEST,
        ));
    }
    if !new.same_recovery_entry(&stored) {
        return Err(fail(
            "bundle recovery entry changed",
            ApiError::INVALID_REQUEST,
        ));
    }
    Ok(())
}

pub async fn passphrase_start(
    State(state): State<AppState>,
    session: AuthSession,
    ApiJson(req): ApiJson<PassphraseStartRequest>,
) -> Result<Response> {
    let mut conn = state.pool.acquire().await?;
    let user = store::find_by_id(&mut conn, &session.user_id)
        .await?
        .ok_or(ApiError::INVALID_TOKEN)?;
    drop(conn);
    let username = username(&user.username)?;
    if let Some(response) = limit_username(&state, &username) {
        return Ok(response);
    }
    let (login, credential_response) = server_login(
        &state,
        Some(&user),
        &req.credential_request,
        username.as_str(),
    )?;
    let registration_response =
        registration_response(&state, &req.registration_request, &username)?;
    let handshake_id = state
        .auth
        .pending
        .insert(Pending::Passphrase {
            state: login,
            user_id: session.user_id,
            session_id: session.session_id,
            opaque_record: user.opaque_record,
        })
        .map_err(pending_full)?;
    Ok(Json(PassphraseStartResponse {
        handshake_id,
        credential_response,
        registration_response,
    })
    .into_response())
}

/// Replaces the record and bundle atomically and signs out every other
/// session; the current one continues.
pub async fn passphrase_finish(
    State(state): State<AppState>,
    session: AuthSession,
    ApiJson(req): ApiJson<PassphraseFinishRequest>,
) -> Result<StatusCode> {
    let Some(Pending::Passphrase {
        state: login,
        user_id,
        session_id,
        opaque_record: checked_record,
    }) = state.auth.pending.take(&req.handshake_id)
    else {
        return Err(fail(
            "unknown or expired handshake",
            ApiError::INVALID_CREDENTIALS,
        ));
    };
    if user_id != session.user_id || session_id != session.session_id {
        return Err(fail(
            "handshake from another session",
            ApiError::INVALID_CREDENTIALS,
        ));
    }
    finish_login(*login, &req.credential_finalization)?;
    let record = opaque_record(&req.registration_upload)?;

    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    check_record_unchanged(&mut tx, &session.user_id, &checked_record).await?;
    check_replacement_bundle(&mut tx, &session.vault_id, &req.key_bundle).await?;
    store::replace_credentials(
        &mut tx,
        &session.user_id,
        &session.vault_id,
        &record,
        &req.key_bundle,
        state.auth.now(),
    )
    .await?;
    sessions::delete_all_for_user(&mut tx, &session.user_id, Some(&session.session_id)).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Compared against when the username is unknown, so the work done doesn't
/// depend on whether the account exists.
const DUMMY_VERIFIER_HASH: [u8; 32] = [0; 32];

pub async fn recover_start(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RecoverStartRequest>,
) -> Result<Json<RecoverStartResponse>> {
    let username = username(&req.username)?;
    if req.recovery_verifier.len() != RECOVERY_VERIFIER_LEN {
        return Err(fail("recovery verifier length", ApiError::INVALID_REQUEST));
    }
    let registration_response =
        registration_response(&state, &req.registration_request, &username)?;

    let mut conn = state.pool.acquire().await?;
    let user = store::find_by_username(&mut conn, username.as_str()).await?;
    let presented = Sha256::digest(&req.recovery_verifier);
    let expected = user
        .as_ref()
        .map_or(&DUMMY_VERIFIER_HASH[..], |u| &u.recovery_verifier_hash[..]);
    let matches = ct_eq(&presented, expected);
    let Some(user) = user.filter(|_| matches) else {
        return Err(fail("recovery verifier", ApiError::INVALID_CREDENTIALS));
    };
    let key_bundle = store::key_bundle(&mut conn, &user.vault_id)
        .await?
        .ok_or(ApiError::INTERNAL)?;
    drop(conn);
    let handshake_id = state
        .auth
        .pending
        .insert(Pending::Recover { user })
        .map_err(pending_full)?;
    Ok(Json(RecoverStartResponse {
        handshake_id,
        key_bundle,
        registration_response,
    }))
}

/// Replaces the record and bundle, ends every existing session, and returns
/// a new one.
pub async fn recover_finish(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RecoverFinishRequest>,
) -> Result<Json<Session>> {
    check_device_name(&req.device_name)?;
    let Some(Pending::Recover { user }) = state.auth.pending.take(&req.handshake_id) else {
        return Err(fail(
            "unknown or expired handshake",
            ApiError::INVALID_CREDENTIALS,
        ));
    };
    let record = opaque_record(&req.registration_upload)?;
    let now = state.auth.now();

    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    check_replacement_bundle(&mut tx, &user.vault_id, &req.key_bundle).await?;
    store::replace_credentials(
        &mut tx,
        &user.user_id,
        &user.vault_id,
        &record,
        &req.key_bundle,
        now,
    )
    .await?;
    sessions::delete_all_for_user(&mut tx, &user.user_id, None).await?;
    let issued = sessions::issue(
        &mut tx,
        &user.user_id,
        &user.vault_id,
        &req.device_name,
        req.session_kind,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(session_body(issued, &user.username, user.is_admin)))
}
