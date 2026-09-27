//! End-to-end tests: user -> entry -> tunnel -> exit -> echo server, on localhost.

use std::time::{Duration, Instant};

use kariz::config::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "test-token-0123456789abcdef";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
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
}

impl Setup {
    const fn tcp(mode: &'static str) -> Self {
        Self {
            mode,
            transport: "tcp",
            mux: false,
            encryption: "auto",
        }
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

    /// `[tunnel]` lines shared by both sides.
    fn tunnel_options(&self) -> String {
        format!(
            "transport = \"{}\"\nencryption = \"{}\"\n[tunnel.mux]\nenabled = {}\n",
            self.transport, self.encryption, self.mux
        )
    }
}

/// One side of the tunnel in its own runtime, like a separate process: dropping it
/// shuts the runtime down, which drops every task and closes every socket the side had.
struct Side {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Side {
    fn start(text: &str) -> Self {
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
    let tunnel_port = free_port();
    let user_port = free_port();
    let mode = setup.mode;
    let (entry_tunnel, exit_tunnel) = match mode {
        "reverse" => (
            format!("listen = \"127.0.0.1:{tunnel_port}\""),
            format!("remote = \"127.0.0.1:{tunnel_port}\"\npool = 2"),
        ),
        _ => (
            format!("remote = \"127.0.0.1:{tunnel_port}\""),
            format!("listen = \"127.0.0.1:{tunnel_port}\""),
        ),
    };
    let (options, exit_options) = (setup.tunnel_options(), exit_setup.tunnel_options());
    let entry = format!(
        r#"
        role = "entry"
        mode = "{mode}"
        [[forward]]
        listen = "127.0.0.1:{user_port}"
        target = "127.0.0.1:{target_port}"
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
