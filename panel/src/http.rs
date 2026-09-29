//! The panel's HTTPS server. It answers only under the secret path; everything else
//! gets the same 404 page a plain nginx gives, so a scanner learns nothing.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use rust_embed::RustEmbed;
use serde_json::json;
use tokio::net::TcpListener;
use tower::Service;
use tracing::{debug, info};

use crate::config::Config;
use crate::db::Db;

/// The web app, built into the binary (see build.rs for the placeholder).
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
}

/// The nginx 404 page.
const NOT_FOUND: &str = "<html>\r\n<head><title>404 Not Found</title></head>\r\n<body>\r\n\
                         <center><h1>404 Not Found</h1></center>\r\n<hr><center>nginx</center>\r\n\
                         </body>\r\n</html>\r\n";

fn not_found() -> Response<Body> {
    let mut response = Response::new(Body::from(NOT_FOUND));
    *response.status_mut() = StatusCode::NOT_FOUND;
    let headers = response.headers_mut();
    headers.insert(header::SERVER, HeaderValue::from_static("nginx"));
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    response
}

/// The panel's routes, all under `/<path>/`.
pub fn router(path: &str, state: AppState) -> Router {
    let inner = Router::new()
        .route("/api/version", get(version))
        .merge(crate::api::routes())
        .route("/api/{*rest}", get(|| async { not_found() }))
        .fallback(get(asset))
        .with_state(state);
    Router::new()
        // `nest` does not hand the address with a trailing slash to the inner router,
        // and that is the address of the panel's front page.
        .route(
            &format!("/{path}/"),
            get(|| async { asset(Uri::from_static("/")).await }),
        )
        .nest(&format!("/{path}"), inner)
        .fallback(|| async { not_found() })
}

async fn version(State(_): State<AppState>) -> impl IntoResponse {
    Json(json!({ "name": "kariz-panel", "version": env!("CARGO_PKG_VERSION") }))
}

/// A file of the web app, or its index page for a path the app handles itself.
async fn asset(uri: Uri) -> Response<Body> {
    let wanted = uri.path().trim_start_matches('/');
    let file = if wanted.is_empty() {
        "index.html"
    } else {
        wanted
    };
    let (name, found) = match Assets::get(file) {
        Some(found) => (file, found),
        // A path with a file extension is a missing file; anything else is a page of
        // the app (it draws itself from the address).
        None if !file.rsplit('/').next().unwrap_or("").contains('.') => {
            match Assets::get("index.html") {
                Some(index) => ("index.html", index),
                None => return not_found(),
            }
        }
        None => return not_found(),
    };
    let mime = mime_of(name);
    let mut response = Response::new(Body::from(found.data.into_owned()));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    // Built files carry a hash in their name and never change; the page itself must
    // always be fetched again.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    // The app loads nothing from anywhere else, and cannot be framed.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
             frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Serves `app` over TLS on `listener` until the task is dropped.
pub async fn serve(listener: TcpListener, config: &Config, app: Router) -> Result<()> {
    let acceptor = kariz::transport::tls::acceptor(&config.cert(), &config.key())
        .context("failed to load the certificate")?;
    let local = listener.local_addr()?;
    info!(address = %local, path = %config.path, "the panel is listening");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let (acceptor, app) = (acceptor.clone(), app.clone());
        tokio::spawn(async move {
            serve_connection(acceptor, stream, peer, app).await;
        });
    }
}

async fn serve_connection(
    acceptor: tokio_rustls::TlsAcceptor,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    app: Router,
) {
    let tls = match acceptor.accept(stream).await {
        Ok(tls) => tls,
        Err(e) => return debug!(%peer, error = %e, "TLS handshake failed"),
    };
    let service = hyper::service::service_fn(move |mut request: axum::http::Request<Incoming>| {
        let mut app = app.clone();
        // The handlers see who is asking (for the lockout and the session list).
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        async move { app.call(request).await }
    });
    if let Err(e) = Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(tls), service)
        .await
    {
        debug!(%peer, error = %e, "connection ended with an error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get_path(path: &str) -> (StatusCode, String, Option<String>) {
        let app = router(
            "k-7f3a9c",
            AppState {
                db: Db::in_memory().unwrap(),
            },
        );
        let response = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned(), cache)
    }

    #[tokio::test]
    async fn only_the_secret_path_answers() {
        for path in [
            "/",
            "/login",
            "/api/version",
            "/k-other/",
            "/k-7f3a9cc/",
            "/admin",
        ] {
            let (status, body, _) = get_path(path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(body.contains("<center>nginx</center>"), "{path}: {body}");
        }
        for path in ["/k-7f3a9c", "/k-7f3a9c/", "/k-7f3a9c/servers"] {
            let (status, body, cache) = get_path(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("Kariz"), "{path}");
            assert_eq!(cache.as_deref(), Some("no-store"), "{path}");
        }
    }

    #[tokio::test]
    async fn the_api_and_missing_files() {
        let (status, body, _) = get_path("/k-7f3a9c/api/version").await;
        assert_eq!(status, StatusCode::OK);
        let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(doc["name"], "kariz-panel");
        // An unknown API path and a missing file are 404s, not the app's page.
        for path in ["/k-7f3a9c/api/nothing", "/k-7f3a9c/assets/missing.js"] {
            assert_eq!(get_path(path).await.0, StatusCode::NOT_FOUND, "{path}");
        }
    }
}
