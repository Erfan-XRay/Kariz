//! End-to-end tests: user -> entry -> tunnel -> exit -> echo server, on localhost.

use std::time::{Duration, Instant};

use kariz::config::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

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

    /// `[tunnel]` lines shared by both sides.
    fn tunnel_options(&self) -> String {
        format!(
            "transport = \"{}\"\nencryption = \"{}\"\n[tunnel.mux]\nenabled = {}\n",
            self.transport, self.encryption, self.mux
        )
    }
}

struct Tunnel {
    user_port: u16,
    tasks: Vec<JoinHandle<anyhow::Result<()>>>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn spawn_side(text: String) -> JoinHandle<anyhow::Result<()>> {
    let config = Config::parse(&text).unwrap_or_else(|e| panic!("{e:#}\n{text}"));
    tokio::spawn(kariz::run(config))
}

async fn start(setup: Setup, entry_token: &str, exit_token: &str, target_port: u16) -> Tunnel {
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
    let options = setup.tunnel_options();
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
        {options}
        "#
    );
    // Start the listening side first so the dialing side connects right away.
    let tasks = if mode == "reverse" {
        let e = spawn_side(entry);
        tokio::time::sleep(Duration::from_millis(100)).await;
        vec![e, spawn_side(exit)]
    } else {
        let x = spawn_side(exit);
        tokio::time::sleep(Duration::from_millis(100)).await;
        vec![x, spawn_side(entry)]
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    Tunnel { user_port, tasks }
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
