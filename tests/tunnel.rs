//! End-to-end tests: user -> entry -> tunnel -> exit -> echo server, on localhost.

use std::time::{Duration, Instant};

use kariz::config::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

const TOKEN: &str = "test-token-0123456789abcdef";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Echoes TCP and UDP on the same port.
async fn echo_server() -> u16 {
    loop {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // The same port number may be taken for UDP; then try another.
        let Ok(udp) = UdpSocket::bind(("127.0.0.1", port)).await else {
            continue;
        };
        // Several clients send 60 KB packets at once: with the kernel's default buffer
        // (about 208 KB) the target itself would drop some, and the tests are strict.
        let _ = socket2::SockRef::from(&udp).set_recv_buffer_size(4 << 20);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65_536];
            while let Ok((n, from)) = udp.recv_from(&mut buf).await {
                let _ = udp.send_to(&buf[..n], from).await;
            }
        });
        return port;
    }
}

/// A UDP target that answers every packet with the source port it came from, so a test
/// can tell whether packets used the same flow (same exit socket).
async fn udp_port_reporter() -> u16 {
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = udp.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((_, from)) = udp.recv_from(&mut buf).await {
            let _ = udp.send_to(&from.port().to_be_bytes(), from).await;
        }
    });
    port
}

/// One way to set up the tunnel. The scenarios below run for every setup listed in
/// `tunnel_tests!`, so a new transport or layer only needs a new row there.
#[derive(Clone, Copy, Debug)]
struct Setup {
    mode: &'static str,
    transport: &'static str,
    mux: bool,
    encryption: &'static str,
    /// `tunnel.ws.path`, for `ws` and `wss`.
    ws_path: &'static str,
    /// `wss` dialer: pin a certificate the listener does not have.
    wrong_pin: bool,
    /// `ws` / `wss` dialer: `tunnel.ws.early_data`.
    early_data: bool,
    /// `tuning.keepalive_secs` (mux ping interval); 0 keeps the profile default.
    keepalive_secs: u64,
    /// `tuning.udp_timeout_secs` / `udp_max_flows`; 0 keeps the default.
    udp_timeout_secs: u64,
    udp_max_flows: usize,
}

impl Setup {
    const fn tcp(mode: &'static str) -> Self {
        Self {
            mode,
            transport: "tcp",
            mux: false,
            encryption: "auto",
            ws_path: "/kariz-e2e",
            wrong_pin: false,
            early_data: false,
            keepalive_secs: 0,
            udp_timeout_secs: 0,
            udp_max_flows: 0,
        }
    }

    const fn udp_timeout(self, udp_timeout_secs: u64) -> Self {
        Self {
            udp_timeout_secs,
            ..self
        }
    }

    const fn udp_max_flows(self, udp_max_flows: usize) -> Self {
        Self {
            udp_max_flows,
            ..self
        }
    }

    const fn keepalive(self, keepalive_secs: u64) -> Self {
        Self {
            keepalive_secs,
            ..self
        }
    }

    const fn early_data(self) -> Self {
        Self {
            early_data: true,
            ..self
        }
    }

    const fn wss(mode: &'static str) -> Self {
        Self {
            transport: "wss",
            ..Self::ws(mode)
        }
    }

    const fn wrong_pin(self) -> Self {
        Self {
            wrong_pin: true,
            ..self
        }
    }

    const fn ws(mode: &'static str) -> Self {
        Self {
            transport: "ws",
            mux: true,
            ..Self::tcp(mode)
        }
    }

    const fn no_mux(self) -> Self {
        Self { mux: false, ..self }
    }

    const fn ws_path(self, ws_path: &'static str) -> Self {
        Self { ws_path, ..self }
    }

    const fn encryption(self, encryption: &'static str) -> Self {
        Self { encryption, ..self }
    }

    const fn mux(self) -> Self {
        Self { mux: true, ..self }
    }

    const fn transport(self, transport: &'static str) -> Self {
        Self { transport, ..self }
    }

    /// Transport lines and sub-tables of `[tunnel]` for the listening or dialing side.
    fn tunnel_options(&self, listening: bool) -> String {
        let mut options = format!(
            "transport = \"{}\"\nencryption = \"{}\"\n[tunnel.mux]\nenabled = {}\n",
            self.transport, self.encryption, self.mux
        );
        if self.transport.starts_with("ws") {
            options += &format!("[tunnel.ws]\npath = \"{}\"\n", self.ws_path);
            if self.early_data && !listening {
                options += "early_data = true\n";
            }
        }
        if self.transport == "wss" {
            let cert = test_cert();
            options += &if listening {
                format!(
                    "[tunnel.tls]\ncert = {:?}\nkey = {:?}\n",
                    cert.cert, cert.key
                )
            } else {
                let pin = if self.wrong_pin {
                    &cert.other_pin
                } else {
                    &cert.pin
                };
                format!("[tunnel.tls]\nsni = \"tunnel.example\"\npin_sha256 = \"{pin}\"\n")
            };
        }
        options += "[tuning]\n";
        if self.keepalive_secs > 0 {
            options += &format!("keepalive_secs = {}\n", self.keepalive_secs);
        }
        if self.udp_timeout_secs > 0 {
            options += &format!("udp_timeout_secs = {}\n", self.udp_timeout_secs);
        }
        if self.udp_max_flows > 0 {
            options += &format!("udp_max_flows = {}\n", self.udp_max_flows);
        }
        options
    }
}

/// A self-signed certificate for the `wss` listener, written once per test run.
struct TestCert {
    cert: std::path::PathBuf,
    key: std::path::PathBuf,
    pin: String,
    /// Pin of a different certificate.
    other_pin: String,
}

fn test_cert() -> &'static TestCert {
    static CERT: std::sync::OnceLock<TestCert> = std::sync::OnceLock::new();
    CERT.get_or_init(|| {
        let generate =
            || rcgen::generate_simple_self_signed(vec!["tunnel.example".into()]).unwrap();
        let (c, other) = (generate(), generate());
        let dir = std::env::temp_dir().join(format!("kariz-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        std::fs::write(&cert, c.cert.pem()).unwrap();
        std::fs::write(&key, c.signing_key.serialize_pem()).unwrap();
        TestCert {
            cert,
            key,
            pin: kariz::transport::tls::cert_sha256(c.cert.der()),
            other_pin: kariz::transport::tls::cert_sha256(other.cert.der()),
        }
    })
}

/// One side of the tunnel in its own runtime, like a separate process: dropping it
/// shuts the runtime down, which drops every task and closes every socket the side had.
struct Side {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Side {
    fn start(text: &str) -> Self {
        if std::env::var_os("KARIZ_TEST_LOG").is_some() {
            let _ = tracing_subscriber::fmt()
                .with_env_filter("kariz=debug")
                .with_thread_names(true)
                .try_init();
        }
        let config = Config::parse(text).unwrap_or_else(|e| panic!("{e:#}\n{text}"));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            rt.spawn(async move {
                if let Err(e) = kariz::run(config).await {
                    eprintln!("tunnel side stopped: {e:#}");
                }
            });
            let _ = stopped.recv();
            rt.shutdown_timeout(Duration::from_secs(2));
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Side {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Tunnel {
    user_port: u16,
    /// Held only to keep a proxy between the two sides running.
    _proxy: Option<Nginx>,
    /// Held only to keep the entry side running.
    _entry: Side,
    exit: Option<Side>,
    exit_config: String,
}

impl Tunnel {
    fn kill_exit(&mut self) {
        drop(self.exit.take());
    }

    fn restart_exit(&mut self) {
        self.exit = Some(Side::start(&self.exit_config));
    }
}

async fn start(setup: Setup, entry_token: &str, exit_token: &str, target_port: u16) -> Tunnel {
    start_pair(setup, setup, entry_token, exit_token, target_port).await
}

/// Like [`start`], with separate settings for each side (for mismatch tests).
async fn start_pair(
    setup: Setup,
    exit_setup: Setup,
    entry_token: &str,
    exit_token: &str,
    target_port: u16,
) -> Tunnel {
    let direct = |port| (port, None);
    start_via(
        setup,
        exit_setup,
        entry_token,
        exit_token,
        target_port,
        direct,
    )
    .await
}

/// Like [`start_pair`], with a proxy between the sides: `proxy` gets the port the
/// listening side uses and returns the port the dialing side should connect to.
async fn start_via(
    setup: Setup,
    exit_setup: Setup,
    entry_token: &str,
    exit_token: &str,
    target_port: u16,
    proxy: impl FnOnce(u16) -> (u16, Option<Nginx>),
) -> Tunnel {
    let tunnel_port = free_port();
    let (dial_port, proxy) = proxy(tunnel_port);
    let user_port = free_port();
    let mode = setup.mode;
    let (entry_tunnel, exit_tunnel) = match mode {
        "reverse" => (
            format!("listen = \"127.0.0.1:{tunnel_port}\""),
            format!("remote = \"127.0.0.1:{dial_port}\"\npool = 2"),
        ),
        _ => (
            format!("remote = \"127.0.0.1:{dial_port}\""),
            format!("listen = \"127.0.0.1:{tunnel_port}\""),
        ),
    };
    let entry_listens = mode == "reverse";
    let options = setup.tunnel_options(entry_listens);
    let exit_options = exit_setup.tunnel_options(!entry_listens);
    let entry = format!(
        r#"
        role = "entry"
        mode = "{mode}"
        [[forward]]
        listen = "127.0.0.1:{user_port}"
        target = "127.0.0.1:{target_port}"
        protocol = "tcp+udp"
        [tunnel]
        {entry_tunnel}
        token = "{entry_token}"
        {options}
        "#
    );
    let exit = format!(
        r#"
        role = "exit"
        mode = "{mode}"
        [tunnel]
        {exit_tunnel}
        token = "{exit_token}"
        {exit_options}
        "#
    );
    // Start the listening side first so the dialing side connects right away.
    let (entry_side, exit_side) = if mode == "reverse" {
        let e = Side::start(&entry);
        tokio::time::sleep(Duration::from_millis(100)).await;
        (e, Side::start(&exit))
    } else {
        let x = Side::start(&exit);
        tokio::time::sleep(Duration::from_millis(100)).await;
        (Side::start(&entry), x)
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    Tunnel {
        user_port,
        _proxy: proxy,
        _entry: entry_side,
        exit: Some(exit_side),
        exit_config: exit,
    }
}

async fn echo_roundtrip(port: u16, payload: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
    let (mut r, mut w) = s.split();
    let write = async {
        w.write_all(payload).await?;
        w.shutdown().await
    };
    let mut out = Vec::with_capacity(payload.len());
    let read = r.read_to_end(&mut out);
    let (a, b) = tokio::join!(write, read);
    a?;
    b?;
    Ok(out)
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

async fn check_echo(setup: Setup) {
    let target = echo_server().await;
    let tunnel = start(setup, TOKEN, TOKEN, target).await;

    // Several concurrent connections, including more than the reverse pool size.
    let mut handles = Vec::new();
    for i in 0..10 {
        let port = tunnel.user_port;
        handles.push(tokio::spawn(async move {
            let payload = pattern(1000 + i * 7919);
            let got = echo_roundtrip(port, &payload).await.unwrap();
            assert_eq!(got, payload);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // Large transfer integrity.
    let payload = pattern(8 * 1024 * 1024);
    let got = echo_roundtrip(tunnel.user_port, &payload).await.unwrap();
    assert!(got == payload, "large payload corrupted");
}

/// The user connection must be closed without passing data, not left hanging.
async fn expect_closed_without_data(tunnel: &Tunnel) {
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        echo_roundtrip(tunnel.user_port, b"hello"),
    )
    .await
    .expect("user connection should be closed, not hang");
    if let Ok(data) = result {
        assert!(data.is_empty(), "no data may pass");
    }
}

async fn check_token_mismatch(setup: Setup) {
    let target = echo_server().await;
    let tunnel = start(setup, TOKEN, "another-token-0123456789", target).await;
    expect_closed_without_data(&tunnel).await;
}

async fn check_unreachable_target(setup: Setup) {
    let dead_port = free_port();
    let tunnel = start(setup, TOKEN, TOKEN, dead_port).await;
    expect_closed_without_data(&tunnel).await;
}

/// The exit side dies in the middle of a transfer: the user connection must end instead
/// of hanging, and once the exit is back, new connections work again.
async fn check_exit_restart(setup: Setup) {
    let target = echo_server().await;
    let mut tunnel = start(setup, TOKEN, TOKEN, target).await;
    assert_eq!(
        echo_roundtrip(tunnel.user_port, b"warm up").await.unwrap(),
        b"warm up"
    );

    let user = TcpStream::connect(("127.0.0.1", tunnel.user_port))
        .await
        .unwrap();
    let (mut r, mut w) = user.into_split();
    let writer = tokio::spawn(async move {
        let chunk = pattern(16 * 1024);
        while w.write_all(&chunk).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let mut buf = vec![0u8; 64 * 1024];
    let mut echoed = 0;
    while echoed < 256 * 1024 {
        echoed += r.read(&mut buf).await.unwrap();
    }

    tunnel.kill_exit();
    let ended = tokio::time::timeout(Duration::from_secs(20), async {
        while matches!(r.read(&mut buf).await, Ok(n) if n > 0) {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "user connection hung after the exit side died"
    );
    writer.abort();

    tunnel.restart_exit();
    let recovered = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let attempt = tokio::time::timeout(
                Duration::from_secs(5),
                echo_roundtrip(tunnel.user_port, b"back again"),
            );
            if let Ok(Ok(data)) = attempt.await {
                if data == b"back again" {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    assert!(
        recovered.is_ok(),
        "tunnel did not recover after the exit side came back"
    );
}

/// A UDP client socket talking to the tunnel's user port.
async fn udp_client(port: u16) -> UdpSocket {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.connect(("127.0.0.1", port)).await.unwrap();
    sock
}

/// Sends `payload` and waits for the answer.
async fn udp_roundtrip(sock: &UdpSocket, payload: &[u8], wait: Duration) -> Option<Vec<u8>> {
    sock.send(payload).await.unwrap();
    let mut buf = vec![0u8; 65_536];
    match tokio::time::timeout(wait, sock.recv(&mut buf)).await {
        Ok(Ok(n)) => Some(buf[..n].to_vec()),
        _ => None,
    }
}

/// Several UDP clients at once, each with its own flow, packets from 1 byte to 60 KB.
async fn check_udp_echo(setup: Setup) {
    let target = echo_server().await;
    let tunnel = start(setup, TOKEN, TOKEN, target).await;
    let mut handles = Vec::new();
    for client in 0..8u8 {
        let port = tunnel.user_port;
        handles.push(tokio::spawn(async move {
            let sock = udp_client(port).await;
            for (i, len) in [1, 100, 1400, 8000, 60_000, 1].into_iter().enumerate() {
                let mut payload = pattern(len);
                payload[0] = client;
                if len > 1 {
                    payload[1] = i as u8;
                }
                let got = udp_roundtrip(&sock, &payload, Duration::from_secs(10))
                    .await
                    .unwrap_or_else(|| panic!("client {client}: no answer for {len} bytes"));
                assert!(
                    got == payload,
                    "client {client}: {len}-byte packet corrupted"
                );
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

macro_rules! tunnel_tests {
    ($($name:ident: $setup:expr;)*) => {
        $(
            mod $name {
                use super::*;

                #[tokio::test(flavor = "multi_thread")]
                async fn echo() {
                    check_echo($setup).await;
                }

                #[tokio::test(flavor = "multi_thread")]
                async fn rejects_wrong_token() {
                    check_token_mismatch($setup).await;
                }

                #[tokio::test(flavor = "multi_thread")]
                async fn unreachable_target_closes_user_connection() {
                    check_unreachable_target($setup).await;
                }

                #[tokio::test(flavor = "multi_thread")]
                async fn recovers_after_exit_restart() {
                    check_exit_restart($setup).await;
                }

                #[tokio::test(flavor = "multi_thread")]
                async fn udp_echo() {
                    check_udp_echo($setup).await;
                }
            }
        )*

        /// Every setup above, for the throughput run.
        const ALL_SETUPS: &[(&str, Setup)] = &[$((stringify!($name), $setup)),*];
    };
}

tunnel_tests! {
    tcp_reverse: Setup::tcp("reverse");
    tcp_direct: Setup::tcp("direct");
    tcp_reverse_chacha: Setup::tcp("reverse").encryption("chacha20-poly1305");
    tcp_direct_aes: Setup::tcp("direct").encryption("aes-256-gcm");
    tcp_reverse_plain: Setup::tcp("reverse").encryption("none");
    tcp_direct_plain: Setup::tcp("direct").encryption("none");
    mux_reverse: Setup::tcp("reverse").mux();
    mux_direct: Setup::tcp("direct").mux();
    tcpmux_reverse_plain: Setup::tcp("reverse").transport("tcpmux").mux().encryption("none");
    tcpmux_direct_chacha: Setup::tcp("direct").transport("tcpmux").mux().encryption("chacha20-poly1305");
    ws_reverse: Setup::ws("reverse");
    ws_direct: Setup::ws("direct");
    ws_reverse_no_mux: Setup::ws("reverse").no_mux();
    ws_direct_no_mux_plain: Setup::ws("direct").no_mux().encryption("none");
    wss_reverse: Setup::wss("reverse");
    wss_direct: Setup::wss("direct");
    wss_reverse_no_mux: Setup::wss("reverse").no_mux();
    wss_direct_no_mux_chacha: Setup::wss("direct").no_mux().encryption("chacha20-poly1305");
    ws_direct_no_mux_early: Setup::ws("direct").no_mux().early_data();
    ws_reverse_early: Setup::ws("reverse").early_data();
    wss_direct_early: Setup::wss("direct").early_data();
}

/// A `wss` dialer that pins another certificate refuses the listener, so nothing passes.
#[tokio::test(flavor = "multi_thread")]
async fn wss_wrong_pin_is_rejected() {
    let target = echo_server().await;
    let runs = ["reverse", "direct"].map(|mode| {
        tokio::spawn(async move {
            let setup = Setup::wss(mode);
            let (entry, exit) = if mode == "reverse" {
                (setup, setup.wrong_pin())
            } else {
                (setup.wrong_pin(), setup)
            };
            let tunnel = start_pair(entry, exit, TOKEN, TOKEN, target).await;
            expect_closed_without_data(&tunnel).await;
        })
    });
    for run in runs {
        run.await.unwrap();
    }
}

/// Both sides must agree on the WebSocket path; the listener answers any other path
/// with a 404, so nothing passes.
#[tokio::test(flavor = "multi_thread")]
async fn ws_path_mismatch_is_rejected() {
    let target = echo_server().await;
    let runs = ["reverse", "direct"].map(|mode| {
        tokio::spawn(async move {
            let entry = Setup::ws(mode);
            let exit = entry.ws_path("/somewhere-else");
            let tunnel = start_pair(entry, exit, TOKEN, TOKEN, target).await;
            expect_closed_without_data(&tunnel).await;
        })
    });
    for run in runs {
        run.await.unwrap();
    }
}

/// Mux must be on or off on both sides; a mismatch fails the handshake, it does not
/// turn into garbage on the wire.
#[tokio::test(flavor = "multi_thread")]
async fn mux_mismatch_is_rejected() {
    let target = echo_server().await;
    let mut cases = Vec::new();
    for mode in ["reverse", "direct"] {
        cases.push((Setup::tcp(mode).mux(), Setup::tcp(mode)));
        cases.push((Setup::tcp(mode), Setup::tcp(mode).mux()));
    }
    let runs = cases.into_iter().map(|(entry, exit)| {
        tokio::spawn(async move {
            let tunnel = start_pair(entry, exit, TOKEN, TOKEN, target).await;
            expect_closed_without_data(&tunnel).await;
        })
    });
    for run in runs.collect::<Vec<_>>() {
        run.await.unwrap();
    }
}

/// Rough localhost throughput check: `cargo test --release -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn throughput() {
    for &(name, setup) in ALL_SETUPS {
        let target = echo_server().await;
        let tunnel = start(setup, TOKEN, TOKEN, target).await;
        let payload = pattern(256 * 1024 * 1024);
        let start = Instant::now();
        let got = echo_roundtrip(tunnel.user_port, &payload).await.unwrap();
        let secs = start.elapsed().as_secs_f64();
        assert_eq!(got.len(), payload.len());
        let mbps = (payload.len() as f64 * 8.0) / secs / 1e6;
        println!("{name}: 256 MiB echoed in {secs:.2}s = {mbps:.0} Mbit/s each way");
    }
}

/// nginx as a stand-in for a CDN: it proxies the WebSocket (optionally terminating TLS)
/// and closes connections idle for `idle_secs`. Killed on drop.
struct Nginx {
    child: std::process::Child,
    port: u16,
    dir: std::path::PathBuf,
}

impl Nginx {
    /// The nginx binary, or `None` to skip nginx tests. `KARIZ_REQUIRE_NGINX=1` (set in
    /// CI) turns a missing nginx into a failure instead.
    fn binary() -> Option<String> {
        let bin = std::env::var("KARIZ_NGINX").unwrap_or_else(|_| "nginx".into());
        let found = std::process::Command::new(&bin)
            .arg("-v")
            .output()
            .is_ok_and(|o| o.status.success());
        if found {
            return Some(bin);
        }
        assert!(
            std::env::var_os("KARIZ_REQUIRE_NGINX").is_none(),
            "nginx not found ({bin}) but KARIZ_REQUIRE_NGINX is set"
        );
        eprintln!("nginx not found, skipping (set KARIZ_NGINX to its path)");
        None
    }

    fn start(bin: &str, upstream: u16, path: &str, tls: bool, idle_secs: u64) -> Self {
        let port = free_port();
        let dir = std::env::temp_dir().join(format!("kariz-nginx-{}-{port}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (listen, certs) = if tls {
            let cert = test_cert();
            (
                format!("{port} ssl"),
                format!(
                    "ssl_certificate {};\nssl_certificate_key {};",
                    cert.cert.display(),
                    cert.key.display()
                ),
            )
        } else {
            (port.to_string(), String::new())
        };
        let d = dir.display();
        let conf = format!(
            r#"
            daemon off;
            master_process off;
            worker_processes 1;
            pid {d}/nginx.pid;
            error_log {d}/error.log info;
            events {{ worker_connections 256; }}
            http {{
                access_log off;
                client_body_temp_path {d}/body;
                proxy_temp_path {d}/proxy;
                fastcgi_temp_path {d}/fastcgi;
                uwsgi_temp_path {d}/uwsgi;
                scgi_temp_path {d}/scgi;
                map $http_upgrade $connection_upgrade {{ default upgrade; '' close; }}
                server {{
                    listen 127.0.0.1:{listen};
                    {certs}
                    location {path} {{
                        proxy_pass http://127.0.0.1:{upstream};
                        proxy_http_version 1.1;
                        proxy_set_header Upgrade $http_upgrade;
                        proxy_set_header Connection $connection_upgrade;
                        proxy_set_header Host $host;
                        proxy_set_header X-Forwarded-For $remote_addr;
                        proxy_buffering off;
                        proxy_read_timeout {idle_secs}s;
                        proxy_send_timeout {idle_secs}s;
                    }}
                }}
            }}
            "#
        );
        let conf_path = dir.join("nginx.conf");
        std::fs::write(&conf_path, conf).unwrap();
        let child = std::process::Command::new(bin)
            .arg("-p")
            .arg(&dir)
            .arg("-e")
            .arg(dir.join("error.log"))
            .arg("-c")
            .arg(&conf_path)
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let ready = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        let log = || std::fs::read_to_string(dir.join("error.log")).unwrap_or_default();
        assert!(ready, "nginx did not start:\n{}", log());
        Self { child, port, dir }
    }
}

impl Drop for Nginx {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Idle time nginx allows before it closes a proxied WebSocket.
const NGINX_IDLE_SECS: u64 = 3;

/// Runs `entry` / `exit` through nginx and keeps one user connection idle for longer
/// than nginx allows. With `survives`, the same connection must still work afterwards
/// (mux pings kept the WebSocket busy); without, it must be broken (the control case,
/// proving the idle timeout is real). New connections must work in both cases.
async fn check_through_nginx(entry: Setup, exit: Setup, survives: bool) {
    let Some(bin) = Nginx::binary() else {
        return;
    };
    let dialer = if entry.mode == "direct" { entry } else { exit };
    let tls = dialer.transport == "wss";
    let path = dialer.ws_path;
    let target = echo_server().await;
    let tunnel = start_via(entry, exit, TOKEN, TOKEN, target, |upstream| {
        let nginx = Nginx::start(&bin, upstream, path, tls, NGINX_IDLE_SECS);
        (nginx.port, Some(nginx))
    })
    .await;

    // Bulk data through the proxy.
    let payload = pattern(4 * 1024 * 1024);
    let got = echo_roundtrip(tunnel.user_port, &payload).await.unwrap();
    assert!(got == payload, "payload corrupted through nginx");

    // UDP rides the same WebSocket sessions.
    let sock = udp_client(tunnel.user_port).await;
    for len in [1, 1400, 30_000] {
        let packet = pattern(len);
        let got = udp_roundtrip(&sock, &packet, Duration::from_secs(10)).await;
        assert!(
            got.as_deref() == Some(&packet[..]),
            "UDP through nginx ({len} bytes)"
        );
    }

    let mut user = TcpStream::connect(("127.0.0.1", tunnel.user_port))
        .await
        .unwrap();
    let mut buf = [0u8; 6];
    user.write_all(b"before").await.unwrap();
    user.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"before");

    tokio::time::sleep(Duration::from_secs(NGINX_IDLE_SECS + 3)).await;
    let after = tokio::time::timeout(Duration::from_secs(5), async {
        user.write_all(b"after!").await?;
        user.read_exact(&mut buf).await?;
        std::io::Result::Ok(buf)
    })
    .await;
    match after {
        Ok(Ok(buf)) => assert!(
            survives,
            "idle connection survived: control case is broken ({buf:?})"
        ),
        _ => assert!(
            !survives,
            "idle connection did not survive nginx's idle timeout"
        ),
    }

    let recovered = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let attempt = tokio::time::timeout(
                Duration::from_secs(5),
                echo_roundtrip(tunnel.user_port, b"again"),
            );
            if let Ok(Ok(data)) = attempt.await {
                if data == b"again" {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    assert!(recovered.is_ok(), "no new connections through nginx");
}

/// The tunnel behind nginx as a CDN stand-in (`cargo test nginx`; CI job `cdn`).
mod nginx {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn ws_direct_survives_idle() {
        let setup = Setup::ws("direct").keepalive(1).early_data();
        check_through_nginx(setup, setup, true).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ws_reverse_survives_idle() {
        let setup = Setup::ws("reverse").keepalive(1).early_data();
        check_through_nginx(setup, setup, true).await;
    }

    /// Like a CDN: TLS from the dialer to nginx, plain WebSocket from nginx to the origin.
    #[tokio::test(flavor = "multi_thread")]
    async fn wss_to_proxy_ws_to_origin() {
        let exit = Setup::ws("direct").keepalive(1);
        let entry = Setup::wss("direct").keepalive(1).early_data();
        check_through_nginx(entry, exit, true).await;
    }

    /// Control: with the default 30 s keepalive nginx does cut the idle connection, so
    /// the cases above prove something.
    #[tokio::test(flavor = "multi_thread")]
    async fn control_idle_connections_are_cut_without_frequent_pings() {
        let setup = Setup::ws("direct");
        check_through_nginx(setup, setup, false).await;
    }
}

/// A quiet flow is closed after `udp_timeout_secs` on both sides: the next packet from
/// the same client opens a new flow, which reaches the target from a new source port.
#[tokio::test(flavor = "multi_thread")]
async fn udp_idle_flows_are_closed() {
    let target = udp_port_reporter().await;
    let runs = [Setup::tcp("direct").mux(), Setup::tcp("reverse")].map(|setup| {
        tokio::spawn(async move {
            let setup = setup.udp_timeout(5);
            let tunnel = start(setup, TOKEN, TOKEN, target).await;
            let sock = udp_client(tunnel.user_port).await;
            let wait = Duration::from_secs(10);
            let first = udp_roundtrip(&sock, b"a", wait)
                .await
                .expect("first answer");
            tokio::time::sleep(Duration::from_secs(2)).await;
            let again = udp_roundtrip(&sock, b"b", wait)
                .await
                .expect("second answer");
            assert_eq!(first, again, "{setup:?}: same flow within the timeout");
            tokio::time::sleep(Duration::from_secs(7)).await;
            let later = udp_roundtrip(&sock, b"c", wait)
                .await
                .expect("answer after idle");
            assert_ne!(first, later, "{setup:?}: new flow after the timeout");
        })
    });
    for run in runs {
        run.await.unwrap();
    }
}

/// Clients beyond `udp_max_flows` are not served; the others keep working.
#[tokio::test(flavor = "multi_thread")]
async fn udp_max_flows_is_enforced() {
    let target = echo_server().await;
    let tunnel = start(
        Setup::tcp("direct").mux().udp_max_flows(2),
        TOKEN,
        TOKEN,
        target,
    )
    .await;
    let wait = Duration::from_secs(5);
    let (a, b, c) = (
        udp_client(tunnel.user_port).await,
        udp_client(tunnel.user_port).await,
        udp_client(tunnel.user_port).await,
    );
    assert!(udp_roundtrip(&a, b"a", wait).await.is_some());
    assert!(udp_roundtrip(&b, b"b", wait).await.is_some());
    let short = Duration::from_secs(2);
    assert!(
        udp_roundtrip(&c, b"c", short).await.is_none(),
        "third flow must be refused"
    );
    assert!(udp_roundtrip(&a, b"a2", wait).await.is_some());
}
