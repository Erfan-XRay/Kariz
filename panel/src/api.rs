//! The panel's JSON API for sign-in and sessions.
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
        .route("/api/servers/panel-addresses", get(panel_addresses))
        .route("/api/servers/remove", post(remove_server))
        // A tunnel with many ports is a bigger body than the other calls take.
        .route(
            "/api/tunnels/check",
            post(tunnel_check).layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .route(
            "/api/tunnels",
            post(tunnel_create).layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .route(
            "/api/tunnels/edit",
            post(tunnel_edit).layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .route("/api/tunnels/control", post(tunnel_control))
        .route("/api/tunnels/delete", post(tunnel_delete))
        .route("/api/op", get(op_status))
        .route("/api/ports", get(server_ports))
        .route("/api/tunnel", get(tunnel_spec))
        .route("/api/history", get(history))
        .route("/api/events", get(events))
        .route("/api/logs", get(tunnel_logs))
        .route("/api/tunnels/speedtest", post(tunnel_speedtest))
        .route("/api/tunnels/cert", post(tunnel_cert))
        .route("/api/tunnels/speedtest/start", post(tunnel_speedtest_start))
        .route("/api/tunnels/speedtest/poll", post(tunnel_speedtest_poll))
        .route("/api/tunnels/speedtest/stop", post(tunnel_speedtest_stop))
        .route("/api/networks", get(networks_list).post(network_create))
        .route("/api/networks/delete", post(network_delete))
        .route("/api/networks/links", post(links_create))
        .route("/api/networks/links/delete", post(link_delete))
        .route("/api/servers/address", post(server_address))
        .route("/api/update", get(update_status))
        .route("/api/update/check", post(update_check))
        .route("/api/update/settings", post(update_settings))
        .route("/api/update/apply", post(update_apply))
        .route("/api/update/servers", post(update_servers))
        .route("/api/backup", post(backup))
        .route(
            "/api/restore",
            post(restore).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
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
    // A password check costs real CPU and memory (that is its point): only a few at once, so
    // a flood of tries from many addresses cannot make the panel unusable.
    static GATE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    let gate = GATE
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)))
        .clone();
    let Ok(_permit) = gate.try_acquire_owned() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
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
    /// The one link transport the agent is to use; none or `auto`: all of them in turn.
    transport: Option<String>,
}

/// The panel's own public addresses, to offer in *Add server*, and its agents ports.
async fn panel_addresses(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    let (v4, v6) = tokio::task::spawn_blocking(crate::join::own_addresses)
        .await
        .unwrap_or((None, None));
    reply(
        StatusCode::OK,
        json!({ "v4": v4, "v6": v6, "agent_port": state.agent_port }),
    )
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
    let transport = body
        .transport
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty() && *x != "auto");
    if !crate::join::valid_host(host)
        || name.is_some_and(|n| !crate::join::valid_name(n))
        || transport.is_some_and(|x| !crate::join::valid_transport(x))
    {
        return error(StatusCode::BAD_REQUEST, "bad_input");
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match state
        .hub
        .create_join_via(name, &format!("{host}:{port}"), transport)
    {
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

// ---- tunnels ----

/// Turns a refusal from the pair code into a response.
fn pair_error(e: anyhow::Error) -> Response {
    let code = format!("{e:#}");
    match code.as_str() {
        "bad_name" | "same_server" | "bad_mode" | "bad_action" => {
            error(StatusCode::BAD_REQUEST, &code)
        }
        "busy" => error(StatusCode::CONFLICT, "busy"),
        _ if code.starts_with("old_agent:") => error(StatusCode::CONFLICT, &code),
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

#[derive(Deserialize)]
struct SpecQuery {
    server: String,
    name: String,
}

/// One side of a tunnel as its server keeps it (no token), for the wizard to edit.
async fn tunnel_spec(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SpecQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match state
        .hub
        .ask_as::<crate::wire::Spec>(&q.server, &crate::wire::Request::TunnelGet { name: q.name })
        .await
    {
        Ok(spec) => reply(StatusCode::OK, json!(spec)),
        Err(_) => error(StatusCode::NOT_FOUND, "no_such_tunnel"),
    }
}

#[derive(Deserialize)]
struct HistoryQuery {
    key: String,
    range: Option<String>,
}

/// One series for a chart: `srv:ID:cpu|mem|rx|tx`, `tun:NAME:rate|conns|rtt`.
async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    let range = crate::history::Range::parse(q.range.as_deref().unwrap_or("1h"));
    let (true, Some(range)) = (crate::history::valid_key(&q.key), range) else {
        return error(StatusCode::BAD_REQUEST, "bad_input");
    };
    match state.hub.history.series(&q.key, range) {
        Ok(points) => reply(StatusCode::OK, json!({ "points": points })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<u32>,
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match state.hub.history.events(q.limit.unwrap_or(100)) {
        Ok(events) => reply(StatusCode::OK, json!({ "events": events })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct LogsQuery {
    name: String,
    lines: Option<u32>,
}

async fn tunnel_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match pair::logs(&state.hub, &q.name, q.lines.unwrap_or(200).clamp(1, 1000)).await {
        Ok(lines) => reply(StatusCode::OK, json!({ "lines": lines })),
        Err(e) => match format!("{e:#}").as_str() {
            "no_such_tunnel" => error(StatusCode::NOT_FOUND, "no_such_tunnel"),
            "bad_name" => error(StatusCode::BAD_REQUEST, "bad_name"),
            _ => internal(e),
        },
    }
}

#[derive(Deserialize)]
struct SpeedBody {
    name: String,
    seconds: Option<u32>,
    streams: Option<u32>,
    udp: Option<bool>,
}

async fn tunnel_speedtest(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<SpeedBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    audit(
        &state,
        &format!("session {}", me.id),
        &ip_of(&peer),
        &format!("ran a speed test on {}", body.name),
    );
    match pair::speedtest(
        &state.hub,
        &body.name,
        body.seconds.unwrap_or(10),
        body.streams.unwrap_or(4),
        body.udp.unwrap_or(true),
    )
    .await
    {
        Ok(r) => reply(
            StatusCode::OK,
            json!({ "ok": r.ok, "error": r.error, "text": r.text, "report": r.report }),
        ),
        Err(e) => match format!("{e:#}").as_str() {
            "no_such_tunnel" => error(StatusCode::NOT_FOUND, "no_such_tunnel"),
            "bad_name" => error(StatusCode::BAD_REQUEST, "bad_name"),
            _ => internal(e),
        },
    }
}

async fn tunnel_speedtest_start(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<SpeedBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    audit(
        &state,
        &format!("session {}", me.id),
        &ip_of(&peer),
        &format!("ran a speed test on {}", body.name),
    );
    match pair::speedtest_start(
        &state.hub,
        &body.name,
        body.seconds.unwrap_or(10),
        body.streams.unwrap_or(4),
        body.udp.unwrap_or(true),
    )
    .await
    {
        Ok(r) => reply(
            StatusCode::OK,
            json!({ "ok": r.ok, "error": r.error, "id": r.id }),
        ),
        Err(e) => match format!("{e:#}").as_str() {
            "no_such_tunnel" => error(StatusCode::NOT_FOUND, "no_such_tunnel"),
            "bad_name" => error(StatusCode::BAD_REQUEST, "bad_name"),
            _ => internal(e),
        },
    }
}

#[derive(Deserialize)]
struct CertBody {
    server: String,
    host: String,
    email: Option<String>,
}

/// Gets a certificate for a wss tunnel's listening server (it can take a minute).
async fn tunnel_cert(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<CertBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    audit(
        &state,
        &format!("session {}", me.id),
        &ip_of(&peer),
        &format!(
            "asked for a certificate for {} on {}",
            body.host, body.server
        ),
    );
    match pair::certificate(&state.hub, &body.server, &body.host, body.email.as_deref()).await {
        Ok(r) => reply(
            StatusCode::OK,
            json!({ "ok": r.ok, "error": r.error, "cert": r.cert, "key": r.key }),
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct SpeedPollBody {
    name: String,
    id: String,
    after: Option<u32>,
}

async fn tunnel_speedtest_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SpeedPollBody>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, true) {
        return r;
    }
    match pair::speedtest_poll(&state.hub, &body.name, &body.id, body.after.unwrap_or(0)).await {
        Ok(r) => reply(
            StatusCode::OK,
            json!({ "ok": r.ok, "error": r.error, "lines": r.lines, "next": r.next, "done": r.done, "report": r.report }),
        ),
        Err(e) => match format!("{e:#}").as_str() {
            "no_such_tunnel" => error(StatusCode::NOT_FOUND, "no_such_tunnel"),
            "bad_name" => error(StatusCode::BAD_REQUEST, "bad_name"),
            _ => internal(e),
        },
    }
}

#[derive(Deserialize)]
struct SpeedStopBody {
    name: String,
    id: String,
}

/// Stops a speed test that is running.
async fn tunnel_speedtest_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SpeedStopBody>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, true) {
        return r;
    }
    match pair::speedtest_stop(&state.hub, &body.name, &body.id).await {
        Ok(()) => reply(StatusCode::OK, json!({ "ok": true })),
        Err(e) => match format!("{e:#}").as_str() {
            "no_such_tunnel" | "no_such_test" => error(StatusCode::NOT_FOUND, "no_such_test"),
            "bad_name" => error(StatusCode::BAD_REQUEST, "bad_name"),
            _ => internal(e),
        },
    }
}

#[derive(Deserialize)]
struct BackupBody {
    passphrase: String,
}

/// The backup file, sealed with the passphrase, as base64 in JSON.
async fn backup(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<BackupBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match crate::backup::export(&state.db, &body.passphrase) {
        Ok(file) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "downloaded a backup",
            );
            use base64::Engine;
            reply(
                StatusCode::OK,
                json!({ "data": base64::engine::general_purpose::STANDARD.encode(file) }),
            )
        }
        Err(e) if format!("{e:#}") == "short_passphrase" => {
            error(StatusCode::BAD_REQUEST, "short_passphrase")
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct RestoreBody {
    passphrase: String,
    data: String,
    #[serde(default)]
    replace: bool,
}

async fn restore(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<RestoreBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    use base64::Engine;
    let Ok(file) = base64::engine::general_purpose::STANDARD.decode(body.data.trim()) else {
        return error(StatusCode::BAD_REQUEST, "not_a_backup");
    };
    match crate::backup::import(&state.db, &body.passphrase, &file, body.replace) {
        Ok(n) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("restored a backup ({n} servers)"),
            );
            reply(StatusCode::OK, json!({ "servers": n, "restart": true }))
        }
        Err(e) => match format!("{e:#}").as_str() {
            code @ ("wrong_passphrase" | "not_a_backup" | "bad_backup") => {
                error(StatusCode::BAD_REQUEST, code)
            }
            "not_empty" => error(StatusCode::CONFLICT, "not_empty"),
            _ => internal(e),
        },
    }
}

// ---- private networks ----

/// Turns a refusal from the network code into a response: the code is the text before a
/// colon (`overlaps_route:frankfurt:10.77.3.0/24` keeps its detail for the browser).
fn net_error(e: anyhow::Error) -> Response {
    let text = format!("{e:#}");
    let code = text.split(':').next().unwrap_or_default();
    match code {
        "bad_name" | "bad_cidr" | "not_private" | "too_small" | "overlaps_route"
        | "overlaps_network" | "bad_input" | "same_server" => error(StatusCode::BAD_REQUEST, &text),
        "name_taken" | "in_use" | "pool_full" | "busy" => error(StatusCode::CONFLICT, &text),
        "no_such_network" | "no_such_link" | "no_such_server" => {
            error(StatusCode::NOT_FOUND, &text)
        }
        _ => internal(e),
    }
}

async fn networks_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    match (
        state.hub.networks.networks(),
        state.hub.networks.links(None),
    ) {
        (Ok(networks), Ok(links)) => reply(
            StatusCode::OK,
            json!({ "networks": networks, "links": links }),
        ),
        (Err(e), _) | (_, Err(e)) => internal(e),
    }
}

#[derive(Deserialize)]
struct NetworkBody {
    name: String,
    cidr: String,
}

async fn network_create(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<NetworkBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let routes = state.hub.routes();
    let named: Vec<(String, Vec<String>)> = state
        .hub
        .snapshot()
        .map(|list| {
            list.into_iter()
                .filter_map(|s| Some((s.name, routes.get(&s.id)?.clone())))
                .collect()
        })
        .unwrap_or_default();
    match state
        .hub
        .networks
        .create_network(&body.name, &body.cidr, &named)
    {
        Ok(n) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("made network {} ({})", n.name, n.cidr),
            );
            reply(StatusCode::OK, json!(n))
        }
        Err(e) => net_error(e),
    }
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

async fn network_delete(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<IdBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.hub.networks.delete_network(&body.id) {
        Ok(true) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "deleted a network",
            );
            reply(StatusCode::OK, json!({}))
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "no_such_network"),
        Err(e) => net_error(e),
    }
}

#[derive(Deserialize)]
struct LinksBody {
    network: String,
    servers: Vec<String>,
    /// Hub and spoke: this server is linked to each of the others (a full mesh without it).
    hub: Option<String>,
}

async fn links_create(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<LinksBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match crate::netops::create_links(&state.hub, &body.network, body.servers, body.hub) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "made network links",
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => net_error(e),
    }
}

async fn link_delete(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<IdBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match crate::netops::delete_link(&state.hub, &body.id).await {
        Ok(()) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "deleted a network link",
            );
            reply(StatusCode::OK, json!({}))
        }
        Err(e) => net_error(e),
    }
}

#[derive(Deserialize)]
struct AddressBody {
    id: String,
    addr: String,
}

/// The address other servers reach a server at (private networks need both ends' addresses).
async fn server_address(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<AddressBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.hub.set_addr(&body.id, &body.addr) {
        Ok(()) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("set the address of server {}", body.id),
            );
            // Links that were waiting for this address can be made now.
            let hub = state.hub.clone();
            tokio::spawn(async move {
                if let Ok(links) = hub.networks.links(None) {
                    for l in links.iter().filter(|l| l.a == body.id || l.b == body.id) {
                        let _ = hub.net_sync(&l.a).await;
                        let _ = hub.net_sync(&l.b).await;
                    }
                }
            });
            reply(StatusCode::OK, json!({}))
        }
        Err(e) => net_error(e),
    }
}

// ---- updating ----

fn update_error(e: anyhow::Error) -> Response {
    let text = format!("{e:#}");
    let code = text.split(':').next().unwrap_or_default();
    match code {
        "not_configured" | "no_update" | "no_systemd" => error(StatusCode::CONFLICT, code),
        "major_needs_confirm" => error(StatusCode::CONFLICT, code),
        "busy" => error(StatusCode::CONFLICT, "busy"),
        _ => internal(e),
    }
}

async fn update_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authenticate(&state, &headers, false) {
        return r;
    }
    reply(StatusCode::OK, state.hub.update_status())
}

async fn update_check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = authenticate(&state, &headers, true) {
        return r;
    }
    // A failed look is part of the status (the browser shows it), not an error page.
    match state.hub.check_update().await {
        Ok(()) => {}
        Err(e) if format!("{e:#}") == "not_configured" => return update_error(e),
        Err(_) => {}
    }
    reply(StatusCode::OK, state.hub.update_status())
}

#[derive(Deserialize)]
struct UpdateOptions {
    channel: Option<String>,
    auto: Option<bool>,
}

async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateOptions>,
) -> Response {
    if let Err(r) = authenticate(&state, &headers, true) {
        return r;
    }
    let channel = match body.channel.as_deref() {
        None => None,
        Some(c) => match crate::update::Channel::parse(c) {
            Some(c) => Some(c),
            None => return error(StatusCode::BAD_REQUEST, "bad_input"),
        },
    };
    match state.hub.set_update_options(channel, body.auto) {
        Ok(()) => reply(StatusCode::OK, state.hub.update_status()),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ApplyBody {
    #[serde(default)]
    confirm_major: bool,
}

async fn update_apply(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<ApplyBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let target = state
        .hub
        .update_status()
        .get("latest")
        .and_then(|l| l.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_owned();
    match crate::updater::start(&state.hub, body.confirm_major) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                &format!("updated the panel to {target}"),
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => update_error(e),
    }
}

#[derive(Deserialize)]
struct ServersBody {
    #[serde(default)]
    restart_tunnels: bool,
}

/// Updates the servers whose agent is older than the panel, one at a time.
async fn update_servers(
    State(state): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<ServersBody>,
) -> Response {
    let me = match authenticate(&state, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match crate::updater::start_servers(&state.hub, body.restart_tunnels) {
        Ok(op) => {
            audit(
                &state,
                &format!("session {}", me.id),
                &ip_of(&peer),
                "updated the other servers",
            );
            reply(StatusCode::ACCEPTED, json!({ "op": op }))
        }
        Err(e) => update_error(e),
    }
}
