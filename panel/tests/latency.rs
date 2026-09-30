//! An agent and a hub over hard networks: a real round trip time (a delaying TCP proxy),
//! and a path that lets TCP connect and then stalls it, where the agent has to move to KCP
//! by itself.

use std::time::Duration;

use kariz_panel::agent::{self, Agent};
use kariz_panel::db::Db;
use kariz_panel::hub::Hub;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

/// Copies `from` to `to`, holding every chunk back `delay`.
async fn pipe(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n) = from.read(&mut buf).await {
            if n == 0
                || tx
                    .send((Instant::now() + delay, buf[..n].to_vec()))
                    .is_err()
            {
                break;
            }
        }
    });
    while let Some((at, data)) = rx.recv().await {
        tokio::time::sleep_until(at).await;
        if to.write_all(&data).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
}

async fn proxy(target: String, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let target = target.clone();
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(target).await else {
                    return;
                };
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                tokio::spawn(pipe(cr, sw, delay));
                tokio::spawn(pipe(sr, cw, delay));
            });
        }
    });
    addr
}

#[tokio::test]
async fn a_server_joins_across_a_slow_link() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kariz_panel=debug,kariz=warn")
        .with_test_writer()
        .try_init();
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), panel_dir.path().to_path_buf());
    let acceptor = kariz::link::Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
        .await
        .unwrap();
    let real = acceptor.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(acceptor));
    // 40 ms each way: an 80 ms round trip.
    let slow = proxy(real, Duration::from_millis(40)).await;

    let code = hub.create_join(Some("far-away"), &slow).unwrap();
    let path = dir.path().join("agent.toml");
    let mut config = agent::enroll_from_code(&code, &path).unwrap();
    config.kariz_dir = dir.path().join("kariz");
    config.save(&path).unwrap();
    let running = tokio::spawn(Agent::new(&path, config).run());

    let mut ok = false;
    for _ in 0..300 {
        let servers = hub.snapshot().unwrap();
        if servers
            .iter()
            .any(|s| !s.local && s.online && s.health.is_some())
        {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    running.abort();
    assert!(ok, "the server never showed as online with its health");
}

/// Passes the panel's first reply (the handshake) and then drops what the panel sends: a
/// TCP path that connects, shakes hands and stalls, like some filtered networks.
async fn stalling_proxy(listener: TcpListener, target: String) {
    while let Ok((client, _)) = listener.accept().await {
        let target = target.clone();
        tokio::spawn(async move {
            let Ok(server) = TcpStream::connect(target).await else {
                return;
            };
            let (mut cr, mut cw) = client.into_split();
            let (mut sr, mut sw) = server.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut cr, &mut sw).await;
            });
            let mut first = true;
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = sr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if std::mem::take(&mut first) && cw.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
    }
}

#[tokio::test]
async fn an_agent_moves_to_kcp_when_tcp_is_stalled() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kariz_panel=debug,kariz=warn")
        .with_test_writer()
        .try_init();
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), panel_dir.path().to_path_buf());
    let token = hub.link_token().unwrap();
    // The hub's real tcpmux listener sits behind the stalling proxy; its kcp listener is on
    // the proxy's port number (UDP), as a panel's two listeners are.
    let tcp = kariz::link::Acceptor::bind("127.0.0.1:0", &token)
        .await
        .unwrap();
    let real = tcp.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(tcp));
    let (kcp, front) = loop {
        let kcp = kariz::link::Acceptor::bind_via(
            "127.0.0.1:0",
            &token,
            kariz::config::TransportKind::Kcp,
        )
        .await
        .unwrap();
        let addr = kcp.local_addr().unwrap();
        if let Ok(front) = TcpListener::bind(addr).await {
            break (kcp, front);
        }
    };
    let addr = front.local_addr().unwrap().to_string();
    tokio::spawn(stalling_proxy(front, real));
    tokio::spawn(hub.clone().serve_agents(kcp));

    let code = hub.create_join(Some("behind-a-filter"), &addr).unwrap();
    let path = dir.path().join("agent.toml");
    let mut config = agent::enroll_from_code(&code, &path).unwrap();
    config.kariz_dir = dir.path().join("kariz");
    config.save(&path).unwrap();
    let running = tokio::spawn(Agent::new(&path, config).run());

    let mut ok = false;
    for _ in 0..900 {
        let servers = hub.snapshot().unwrap();
        if servers
            .iter()
            .any(|s| !s.local && s.online && s.health.is_some())
        {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "the server never came online over kcp");
    // Once it has served enough requests, kcp is remembered for the next start.
    let remembered = dir.path().join("link-transport");
    for _ in 0..100 {
        if std::fs::read_to_string(&remembered).is_ok_and(|t| t == "kcp") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    running.abort();
    assert_eq!(std::fs::read_to_string(&remembered).unwrap(), "kcp");
    // Just one server: the stalled tries did not leave half-registered ones behind.
    assert_eq!(
        hub.snapshot().unwrap().iter().filter(|s| !s.local).count(),
        1
    );
}
