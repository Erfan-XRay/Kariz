//! The panel's JSON API for sign-in and sessions (docs/PHASE11.md, section 3).
//!
//! * The session cookie is `__Host-kariz` (`Secure`, `HttpOnly`, `SameSite=Strict`,
//!   `Path=/`); its value is a random token of which only a hash is stored.
//! * Every request that changes something needs the header `X-Kariz-CSRF` with the value
//!   the session was given (`GET /api/session` shows it to the page). Sign-in requests
//!   have no session yet; they need `Content-Type: application/json`, which a page on
//!   another site cannot send without the browser asking first.
//! * Failed tries are counted per address; five lock it out for 15 minutes.

// Handlers leave early with a ready `Response` as the `Err`; that is how axum code is
// usually written, and the response is built once, on a slow path.
#![allow(clippy::result_large_err)]

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, Session};
use crate::http::AppState;
use crate::pair::{self, PairRequest};

const COOKIE: &str = "__Host-kariz";
const CSRF_HEADER: &str = "x-kariz-csrf";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/link", post(link_login))
        .route("/api/logout", post(logout))
        .route("/api/servers", get(servers))
        .route("/api/servers/join-code", post(join_code))
        .route("/api/servers/remove", post(remove_server))
        .route("/api/tunnels/check", post(tunnel_check))
        .route("/api/tunnels", post(tunnel_create))
        .route("/api/tunnels/edit", post(tunnel_edit))
        .route("/api/tunnels/control", post(tunnel_control))
        .route("/api/tunnels/delete", post(tunnel_delete))
        .route("/api/op", get(op_status))
        .route("/api/ports", get(server_ports))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/revoke", post(revoke))
        .route("/api/password", post(password))
        .route("/api/links", post(new_link))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

type Peer = Option<Extension<ConnectInfo<SocketAddr>>>;

fn ip_of(peer: &Peer) -> String {
    peer.as_ref().map_or_else(
        || "unknown".to_owned(),
        |Extension(ConnectInfo(a))| a.ip().to_string(),
    )
}

fn agent_of(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn reply(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &str) -> Response {
    reply(status, json!({ "error": code }))
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "api error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
}

fn set_cookie(token: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age}"
    ))
    .expect("a cookie made of hex")
}

/// The signed-in session of a request. With `csrf`, the request must also carry the
/// session's CSRF value.
fn authenticate(state: &AppState, headers: &HeaderMap, csrf: bool) -> Result<Session, Response> {
    let token = cookie_value(headers, COOKIE)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "signed_out"))?;
    let session = auth::find_session(&state.db, &token, auth::now())
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "signed_out"))?;
    if csrf {
        let sent = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
        if sent != Some(session.csrf.as_str()) {
            return Err(error(StatusCode::FORBIDDEN, "csrf"));
        }
    }
    Ok(session)
}

fn audit(state: &AppState, who: &str, ip: &str, what: &str) {
    if let Err(e) = state.db.audit(who, Some(ip), what) {
        tracing::warn!(error = %e, "could not write the audit log");
    }
}

/// Starts a session for a request that just proved itself.
fn sign_in(state: &AppState, ip: &str, headers: &HeaderMap, how: &str) -> Response {
    let now = auth::now();
    if let Err(e) = auth::clear_failures(&state.db, ip) {
        return internal(e);
    }
    let made = match auth::create_session(&state.db, ip, &agent_of(headers), now) {
        Ok(made) => made,
        Err(e) => return internal(e),
    };
    audit(state, "anonymous", ip, &format!("signed in ({how})"));
    let mut response = reply(StatusCode::OK, json!({ "csrf": made.csrf }));
    response.headers_mut().insert(
        header::SET_COOKIE,
        set_cookie(&made.token, auth::SESSION_IDLE),
    );
    response
}

/// The answer to a failed try, counting it first.
fn refuse(state: &AppState, ip: &str, what: &str) -> Response {
    match auth::record_failure(&state.db, ip, auth::now()) {
        Ok(left) => {
            audit(state, "anonymous", ip, &format!("failed sign-in ({what})"));
            reply(
                StatusCode::UNAUTHORIZED,
                json!({ "error": "wrong", "tries_left": left }),
            )
        }
        Err(e) => internal(e),
    }
}

fn locked(state: &AppState, ip: &str) -> Result<(), Response> {
    match auth::locked_out(&state.db, ip, auth::now()).map_err(internal)? {
        Some(secs) => Err(reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "error": "locked", "retry_after": secs }),
        )),
        None => Ok(()),
    }
}

async fn session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let has_password = auth::has_password(&state.db).unwrap_or(false);
    match authenticate(&state, &headers, false) {
        Ok(s) => reply(
            StatusCode::OK,
            json!({ "authenticated": true, "csrf": s.csrf, "has_password": has_password }),
        ),
        Err(_) => reply(
            StatusCode::OK,
            json!({ "authenticated": false, "has_password": has_password }),
        ),
    }
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let ip = ip_of(&peer);
    if let Err(r) = locked(&state, &ip) {
        return r;
    }
    let db = state.db.clone();
    let password = body.password;
    let ok = tokio::task::spawn_blocking(move || auth::check_password(&db, &password))
        .await
        .map_err(internal)
        .and_then(|r| r.map_err(internal));
    match ok {
        Ok(true) => sign_in(&state, &ip, &headers, "password"),
        Ok(false) => refuse(&state, &ip, "password"),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct LinkBody {
    token: String,
}

async fn link_login(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<LinkBody>,
) -> Response {
    let ip = ip_of(&peer);
    if let Err(r) = locked(&state, &ip) {
        return r;
    }
    match auth::spend_link(&state.db, body.token.trim(), auth::now()) {
        Ok(true) => sign_in(&state, &ip, &headers, "login link"),
        Ok(false) => refuse(&state, &ip, "login link"),
        Err(e) => internal(e),
    }
}

async fn logout(State(state): State<AppState>, peer: Peer, headers: HeaderMap) -> Response {
    let session = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let _ = auth::revoke_session(&state.db, session.id);
    audit(
        &state,
        &format!("session {}", session.id),
        &ip_of(&peer),
        "signed out",
    );
    let mut response = reply(StatusCode::OK, json!({}));
    response
        .headers_mut()
        .insert(header::SET_COOKIE, set_cookie("", 0));
    response
}

async fn sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let me = match authenticate(&state, &headers, false) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match auth::list_sessions(&state.db) {
        Ok(list) => reply(
            StatusCode::OK,
            json!({ "sessions": list.iter().map(|s| json!({
                "id": s.id,
                "ip": s.ip,
                "agent": s.agent,
                "created": s.created,
                "last_seen": s.last_seen,
                "current": s.id == me.id,
            })).collect::<Vec<_>>() }),
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct RevokeBody {
    id: i64,
}

async fn revoke(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<RevokeBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if body.id == me.id {
        return error(StatusCode::BAD_REQUEST, "current_session");
    }
    match auth::revoke_session(&state.db, body.id) {
        Ok(true) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("revoked session {}", body.id),
            );
            reply(StatusCode::OK, json!({}))
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "no_such_session"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct PasswordBody {
    /// Needed when a password is set already.
    current: Option<String>,
    new: String,
}

async fn password(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let ip = ip_of(&peer);
    if let Err(r) = locked(&state, &ip) {
        return r;
    }
    if body.new.chars().count() < auth::MIN_PASSWORD {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "too_short", "min": auth::MIN_PASSWORD }),
        );
    }
    let db = state.db.clone();
    let done = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<()>> {
        if auth::has_password(&db)?
            && !auth::check_password(&db, body.current.as_deref().unwrap_or(""))?
        {
            return Ok(None);
        }
        auth::set_password(&db, &body.new, Some(me.id))?;
        Ok(Some(()))
    })
    .await
    .map_err(internal)
    .and_then(|r| r.map_err(internal));
    match done {
        Ok(Some(())) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip,
                "changed the password",
            );
            reply(StatusCode::OK, json!({}))
        }
        Ok(None) => refuse(&state, &ip, "password change"),
        Err(r) => r,
    }
}

async fn new_link(State(state): State<AppState>, peer: Peer, headers: HeaderMap) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match auth::create_link(&state.db, auth::now()) {
        Ok(token) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "made a login link",
            );
            reply(
                StatusCode::OK,
                json!({ "token": token, "valid_for": auth::LINK_TTL }),
            )
        }
        Err(e) => internal(e),
    }
}

/// The servers the panel knows: its own and every connected agent's, with their health and
/// tunnels as of the last time they were asked (every couple of seconds).
async fn servers(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match state.hub.snapshot() {
        Ok(list) => reply(
            StatusCode::OK,
            json!({ "servers": list, "agents": state.agent_port.is_some() }),
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct JoinBody {
    name: Option<String>,
    /// The address the browser reached the panel by: what the new server should dial.
    host: String,
}

async fn join_code(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<JoinBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(port) = state.agent_port else {
        return error(StatusCode::CONFLICT, "agents_off");
    };
    let host = body.host.trim();
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if !crate::join::valid_host(host) || name.is_some_and(|n| !crate::join::valid_name(n)) {
        return error(StatusCode::BAD_REQUEST, "bad_input");
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match state.hub.create_join(name, &format!("{host}:{port}")) {
        Ok(code) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "made a join code",
            );
            reply(
                StatusCode::OK,
                json!({ "code": code, "valid_for": crate::join::JOIN_TTL }),
            )
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct RemoveBody {
    id: String,
}

async fn remove_server(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<RemoveBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.hub.remove(&body.id) {
        Ok(true) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("removed server {}", body.id),
            );
            reply(StatusCode::OK, json!({}))
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "no_such_server"),
        Err(e) => internal(e),
    }
}

// ---- tunnels (docs/PHASE12.md) ----

/// Turns a refusal from the pair code into a response.
fn pair_error(e: anyhow::Error) -> Response {
    let code = format!("{e:#}");
    match code.as_str() {
        "bad_name" | "same_server" | "bad_mode" | "bad_action" => {
            error(StatusCode::BAD_REQUEST, &code)
        }
        "busy" => error(StatusCode::CONFLICT, "busy"),
        _ => internal(e),
    }
}

async fn tunnel_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, true) {
        return r;
    }
    match pair::check(&state.hub, &body).await {
        Ok((entry, exit)) => reply(StatusCode::OK, json!({ "entry": entry, "exit": exit })),
        Err(e) => pair_error(e),
    }
}

async fn tunnel_create(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let name = body.name.clone();
    match pair::create(&state.hub, body) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("made tunnel {name}"),
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => pair_error(e),
    }
}

async fn tunnel_edit(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let what = format!(
        "edited tunnel {}{}",
        body.name,
        if body.rotate { " (new token)" } else { "" }
    );
    match pair::edit(&state.hub, body) {
        Ok(op) => {
            audit(&state, &format!("session {}", me.id), &ip_of(&peer), &what);
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => pair_error(e),
    }
}

#[derive(Deserialize)]
struct ControlBody {
    name: String,
    action: String,
}

async fn tunnel_control(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<ControlBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match pair::control(&state.hub, &body.name, &body.action) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("{} tunnel {}", body.action, body.name),
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => pair_error(e),
    }
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn tunnel_delete(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<NameBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match pair::delete(&state.hub, &body.name) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("deleted tunnel {}", body.name),
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => pair_error(e),
    }
}

#[derive(Deserialize)]
struct IdQuery {
    id: String,
}

async fn op_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<IdQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match state.hub.ops.get(&q.id) {
        Some(op) => reply(StatusCode::OK, json!(op)),
        None => error(StatusCode::NOT_FOUND, "no_such_operation"),
    }
}

#[derive(Deserialize)]
struct ServerQuery {
    server: String,
}

/// The listening ports of a server, for the wizard to show what is taken.
async fn server_ports(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ServerQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match state
        .hub
        .ask_as::<Vec<crate::wire::PortOwner>>(&q.server, &crate::wire::Request::Ports)
        .await
    {
        Ok(ports) => reply(StatusCode::OK, json!({ "ports": ports })),
        Err(_) => error(StatusCode::NOT_FOUND, "server_offline"),
    }
}
