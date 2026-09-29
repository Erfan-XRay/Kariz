//! The panel over real TLS: set up, served, and asked with the core's own pinned client.

use std::time::Duration;

use kariz::config::TlsConfig;
use kariz::transport::tls::Client;
use kariz_panel::config::Config;
use kariz_panel::db::Db;
use kariz_panel::http::{self, AppState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Running {
    addr: std::net::SocketAddr,
    config: Config,
    fingerprint: String,
    _dir: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let installed = kariz_panel::init(
        &dir.path().join("panel.toml"),
        &dir.path().join("data"),
        Some(1),
    )
    .unwrap();
    let config = installed.config;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http::router(
        &config.path,
        AppState {
            db: Db::open(&config.database()).unwrap(),
        },
    );
    let served = config.clone();
    let task = tokio::spawn(async move {
        let _ = http::serve(listener, &served, app).await;
    });
    Running {
        addr,
        config,
        fingerprint: installed.fingerprint,
        _dir: dir,
        task,
    }
}

/// One HTTPS request; returns the status code and the whole response text.
async fn get(server: &Running, pin: &str, path: &str) -> std::io::Result<(u16, String)> {
    let tls = TlsConfig {
        pin_sha256: Some(pin.to_owned()),
        ..TlsConfig::default()
    };
    let client = Client::new(Some(&tls), "kariz-panel")?;
    let tcp = TcpStream::connect(server.addr).await?;
    let mut stream = client.connect(tcp).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: panel\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut text = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut text)).await;
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((status, text))
}

#[tokio::test]
async fn the_panel_answers_over_tls_under_its_secret_path_only() {
    let server = start().await;
    let path = server.config.path.clone();

    let (status, text) = get(&server, &server.fingerprint, &format!("/{path}/"))
        .await
        .unwrap();
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("Kariz"), "{text}");
    assert!(
        text.to_ascii_lowercase()
            .contains("content-security-policy"),
        "{text}"
    );

    let (status, text) = get(
        &server,
        &server.fingerprint,
        &format!("/{path}/api/version"),
    )
    .await
    .unwrap();
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("\"kariz-panel\""), "{text}");

    for wrong in ["/", "/admin", "/k-000000/", "/api/version"] {
        let (status, text) = get(&server, &server.fingerprint, wrong).await.unwrap();
        assert_eq!(status, 404, "{wrong}");
        assert!(text.contains("<center>nginx</center>"), "{wrong}: {text}");
    }
}

#[tokio::test]
async fn the_wrong_certificate_and_plain_http_get_nothing() {
    let server = start().await;
    let other = "00".repeat(32);
    assert!(
        get(&server, &other, "/").await.is_err(),
        "a wrong pin must fail"
    );

    // Plain HTTP to the TLS port: no page, just a closed connection.
    let mut tcp = TcpStream::connect(server.addr).await.unwrap();
    tcp.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf)).await;
    assert!(
        !String::from_utf8_lossy(&buf).contains("HTTP/1.1 200"),
        "{buf:?}"
    );
}
