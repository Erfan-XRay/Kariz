//! Sign-in through the router, as a browser would use it: cookies, the CSRF header,
//! links, the lockout, sessions.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use kariz_panel::auth;
use kariz_panel::db::Db;
use kariz_panel::http::{self, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const PATH: &str = "k-7f3a9c";

struct Panel {
    app: Router,
    db: Db,
}

/// One "browser": its address and its cookie.
struct Browser {
    ip: &'static str,
    cookie: Option<String>,
    csrf: Option<String>,
}

impl Panel {
    fn new() -> Self {
        let db = Db::in_memory().unwrap();
        Self {
            app: http::router(PATH, AppState::new(db.clone())),
            db,
        }
    }

    fn browser(&self, ip: &'static str) -> Browser {
        Browser {
            ip,
            cookie: None,
            csrf: None,
        }
    }
}

impl Browser {
    /// A request; returns the status, the JSON body and the `Set-Cookie` header.
    async fn call(
        &mut self,
        panel: &Panel,
        method: &str,
        path: &str,
        body: Option<Value>,
        csrf: bool,
    ) -> (StatusCode, Value, Option<String>) {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/{PATH}{path}"))
            .header(header::USER_AGENT, "test browser");
        if let Some(cookie) = &self.cookie {
            request = request.header(header::COOKIE, format!("__Host-kariz={cookie}"));
        }
        if csrf {
            if let Some(csrf) = &self.csrf {
                request = request.header("x-kariz-csrf", csrf);
            }
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        };
        let mut request = request.unwrap();
        let addr: SocketAddr = format!("{}:50000", self.ip).parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(addr));
        let response = panel.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        // Like a browser: keep the cookie the panel sets (or drops), and the CSRF value.
        if let Some(c) = &set_cookie {
            let value = c
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("__Host-kariz=");
            self.cookie = (!value.is_empty()).then(|| value.to_owned());
        }
        if let Some(csrf) = value.get("csrf").and_then(Value::as_str) {
            self.csrf = Some(csrf.to_owned());
        }
        (status, value, set_cookie)
    }
}

#[tokio::test]
async fn a_password_signs_in_and_the_cookie_is_hardened() {
    let panel = Panel::new();
    auth::set_password(&panel.db, "correct horse battery", None).unwrap();
    let mut b = panel.browser("10.0.0.1");

    let (status, doc, _) = b.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["authenticated"], false);
    assert_eq!(doc["has_password"], true);
    assert!(doc.get("csrf").is_none());

    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "nope nope nope"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(doc["error"], "wrong");
    assert_eq!(doc["tries_left"], 4);

    let (status, doc, cookie) = b
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "correct horse battery"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let cookie = cookie.expect("a cookie");
    for part in [
        "__Host-kariz=",
        "Path=/",
        "Secure",
        "HttpOnly",
        "SameSite=Strict",
    ] {
        assert!(cookie.contains(part), "{cookie}");
    }
    assert!(!cookie.contains("Domain"), "{cookie}");

    let (_, doc, _) = b.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(doc["authenticated"], true);
    assert_eq!(doc["csrf"], b.csrf.clone().unwrap());

    // The sign-in was written to the audit log; the password never is.
    let lines: Vec<String> = {
        let conn = panel.db.conn();
        let mut stmt = conn.prepare("SELECT what FROM audit ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(lines, ["failed sign-in (password)", "signed in (password)"]);
}

#[tokio::test]
async fn changes_need_the_csrf_value_and_a_json_body() {
    let panel = Panel::new();
    let mut b = panel.browser("10.0.0.2");
    let token = auth::create_link(&panel.db, auth::now()).unwrap();
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/link",
            Some(json!({"token": token})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Signed in, but without the CSRF header (a page of another site could not add it).
    let (status, doc, _) = b.call(&panel, "POST", "/api/links", None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(doc["error"], "csrf");
    // With a wrong one.
    b.csrf = Some("f".repeat(64));
    let (status, ..) = b.call(&panel, "POST", "/api/links", None, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // With the right one.
    let (_, doc, _) = b.call(&panel, "GET", "/api/session", None, false).await;
    b.csrf = doc["csrf"].as_str().map(str::to_owned);
    let (status, doc, _) = b.call(&panel, "POST", "/api/links", None, true).await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["token"].as_str().unwrap().len(), 64);
    assert_eq!(doc["valid_for"], 3600);

    // A form post (not JSON) to a sign-in address is refused before it is read.
    let request = Request::post(format!("/{PATH}/api/login"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("password=x"))
        .unwrap();
    let response = panel.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn a_login_link_works_once_and_a_signed_out_panel_refuses_the_rest() {
    let panel = Panel::new();
    let token = auth::create_link(&panel.db, auth::now()).unwrap();
    let mut first = panel.browser("10.0.0.3");
    let (status, ..) = first
        .call(
            &panel,
            "POST",
            "/api/link",
            Some(json!({"token": &token})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // The same link in a second browser: spent.
    let mut second = panel.browser("10.0.0.4");
    let (status, doc, _) = second
        .call(
            &panel,
            "POST",
            "/api/link",
            Some(json!({"token": &token})),
            false,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("wrong"))
    );

    for (method, path) in [
        ("GET", "/api/sessions"),
        ("POST", "/api/logout"),
        ("POST", "/api/links"),
    ] {
        let (status, doc, _) = second.call(&panel, method, path, None, true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(doc["error"], "signed_out");
    }

    // Signing out ends the session for good, and drops the cookie.
    let (status, _, cookie) = first.call(&panel, "POST", "/api/logout", None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.unwrap().contains("Max-Age=0"));
    let (_, doc, _) = first.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(doc["authenticated"], false);
}

#[tokio::test]
async fn five_failures_lock_an_address_out_even_for_the_right_password() {
    let panel = Panel::new();
    auth::set_password(&panel.db, "correct horse battery", None).unwrap();
    let mut b = panel.browser("10.0.0.5");
    for left in [4, 3, 2, 1, 0] {
        let (status, doc, _) = b
            .call(
                &panel,
                "POST",
                "/api/login",
                Some(json!({"password": "wrong wrong wrong"})),
                false,
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(doc["tries_left"], left);
    }
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "correct horse battery"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{doc}");
    assert_eq!(doc["error"], "locked");
    assert!(doc["retry_after"].as_i64().unwrap() > 800);
    // Links are locked too (a guessed token is a try).
    let link = auth::create_link(&panel.db, auth::now()).unwrap();
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/link",
            Some(json!({"token": link})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Another address is not.
    let mut other = panel.browser("10.0.0.6");
    let (status, ..) = other
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "correct horse battery"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn sessions_are_listed_revoked_and_ended_by_a_new_password() {
    let panel = Panel::new();
    auth::set_password(&panel.db, "first password here", None).unwrap();
    let mut laptop = panel.browser("10.1.1.1");
    let mut phone = panel.browser("10.2.2.2");
    for b in [&mut laptop, &mut phone] {
        let (status, ..) = b
            .call(
                &panel,
                "POST",
                "/api/login",
                Some(json!({"password": "first password here"})),
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (_, doc, _) = laptop
        .call(&panel, "GET", "/api/sessions", None, false)
        .await;
    let list = doc["sessions"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let mine = list.iter().find(|s| s["current"] == true).unwrap();
    assert_eq!(mine["ip"], "10.1.1.1");
    let theirs = list.iter().find(|s| s["current"] == false).unwrap()["id"].clone();

    // The current session cannot be revoked from itself (that is signing out).
    let (status, ..) = laptop
        .call(
            &panel,
            "POST",
            "/api/sessions/revoke",
            Some(json!({"id": mine["id"]})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, ..) = laptop
        .call(
            &panel,
            "POST",
            "/api/sessions/revoke",
            Some(json!({"id": theirs})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, doc, _) = phone.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(doc["authenticated"], false, "the phone was signed out");

    // A new password needs the current one, is not too short, and ends other sessions.
    let (status, ..) = phone
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "first password here"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, doc, _) = laptop
        .call(
            &panel,
            "POST",
            "/api/password",
            Some(json!({"current": "wrong", "new": "second password here"})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("wrong"))
    );
    let (status, doc, _) = laptop
        .call(
            &panel,
            "POST",
            "/api/password",
            Some(json!({"current": "first password here", "new": "short"})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("too_short"))
    );
    let (status, ..) = laptop
        .call(
            &panel,
            "POST",
            "/api/password",
            Some(json!({"current": "first password here", "new": "second password here"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, doc, _) = laptop
        .call(&panel, "GET", "/api/session", None, false)
        .await;
    assert_eq!(
        doc["authenticated"], true,
        "the session that changed it stays"
    );
    let (_, doc, _) = phone.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(doc["authenticated"], false);
    let (status, ..) = phone
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": "second password here"})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn without_a_password_a_signed_in_session_can_set_the_first_one() {
    let panel = Panel::new();
    let mut b = panel.browser("10.3.3.3");
    let (_, doc, _) = b.call(&panel, "GET", "/api/session", None, false).await;
    assert_eq!(doc["has_password"], false);
    // No password: the password sign-in never succeeds.
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/login",
            Some(json!({"password": ""})),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let link = auth::create_link(&panel.db, auth::now()).unwrap();
    b.call(
        &panel,
        "POST",
        "/api/link",
        Some(json!({"token": link})),
        false,
    )
    .await;
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/password",
            Some(json!({"new": "my brand new password"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(auth::check_password(&panel.db, "my brand new password").unwrap());
}

#[tokio::test]
async fn the_server_list_needs_a_session_and_names_this_server() {
    let panel = Panel::new();
    let mut b = panel.browser("10.4.4.4");
    let link = auth::create_link(&panel.db, auth::now()).unwrap();
    b.call(
        &panel,
        "POST",
        "/api/link",
        Some(json!({"token": link})),
        false,
    )
    .await;
    let (status, doc, _) = b.call(&panel, "GET", "/api/servers", None, false).await;
    assert_eq!(status, StatusCode::OK);
    let servers = doc["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["local"], true);
    assert!(!servers[0]["name"].as_str().unwrap().is_empty());
    assert_eq!(servers[0]["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn a_backup_is_downloaded_locked_and_restored_through_the_api() {
    let panel = Panel::new();
    auth::set_password(&panel.db, "correct horse battery", None).unwrap();
    panel
        .db
        .conn()
        .execute(
            "INSERT INTO servers (id, name, key, created) VALUES ('a1', 'frankfurt', 'k1', 100)",
            [],
        )
        .unwrap();
    let mut anon = panel.browser("10.0.0.9");
    let (status, ..) = anon
        .call(
            &panel,
            "POST",
            "/api/backup",
            Some(json!({"passphrase": "long enough passphrase"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut b = panel.browser("10.0.0.1");
    b.call(
        &panel,
        "POST",
        "/api/login",
        Some(json!({"password": "correct horse battery"})),
        false,
    )
    .await;
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/backup",
            Some(json!({"passphrase": "short"})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("short_passphrase"))
    );

    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/backup",
            Some(json!({"passphrase": "long enough passphrase"})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let data = doc["data"].as_str().unwrap().to_owned();

    // a panel that has servers wants to be told to replace them
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/restore",
            Some(json!({"passphrase": "long enough passphrase", "data": data})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::CONFLICT, Some("not_empty"))
    );
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/restore",
            Some(json!({"passphrase": "not the passphrase", "data": data, "replace": true})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("wrong_passphrase"))
    );

    panel.db.conn().execute("DELETE FROM servers", []).unwrap();
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/restore",
            Some(json!({"passphrase": "long enough passphrase", "data": data})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["servers"], 1);
    assert_eq!(doc["restart"], true);

    // the restore takes a body bigger than the other calls do, but not an enormous one
    let big = "A".repeat(200 * 1024);
    let (status, doc, _) = b
        .call(
            &panel,
            "POST",
            "/api/restore",
            Some(json!({"passphrase": "long enough passphrase", "data": big})),
            true,
        )
        .await;
    assert_eq!(
        (status, doc["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("not_a_backup"))
    );
    let huge = "A".repeat(2 * 1024 * 1024);
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/restore",
            Some(json!({"passphrase": "long enough passphrase", "data": huge})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_tunnel_with_many_ports_gets_through_but_a_huge_body_does_not() {
    let panel = Panel::new();
    auth::set_password(&panel.db, "correct horse battery", None).unwrap();
    let mut b = panel.browser("10.0.0.1");
    b.call(
        &panel,
        "POST",
        "/api/login",
        Some(json!({"password": "correct horse battery"})),
        false,
    )
    .await;
    let forwards: Vec<Value> = (0..200)
        .map(|i| json!({"listen": format!("0.0.0.0:{}", 10000 + i), "target": format!("127.0.0.1:{}", 10000 + i), "protocol": "tcp+udp"}))
        .collect();
    let body = |pad: usize| {
        json!({
            "name": "big", "entry": "local", "exit": "nobody", "mode": "reverse", "transport": "tcpmux",
            "listen": "0.0.0.0:3080", "dial": "203.0.113.5:3080", "forwards": forwards, "pad": "x".repeat(pad)
        })
    };
    // 200 forwards are about 20 KB: more than the other calls take, and accepted here (the
    // servers named are not there, so it is refused for that, not for its size)
    let (status, ..) = b
        .call(&panel, "POST", "/api/tunnels/check", Some(body(0)), true)
        .await;
    assert_ne!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "the ports of a real tunnel must fit"
    );
    // a body of hundreds of kilobytes is refused whatever it says
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/tunnels/check",
            Some(body(200_000)),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    // and the small limit still holds for everything else
    let (status, ..) = b
        .call(
            &panel,
            "POST",
            "/api/servers/remove",
            Some(json!({"id": "x".repeat(20_000)})),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
