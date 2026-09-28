use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::config::Profile;

fn params(key: u8) -> KcpParams {
    KcpParams {
        config: KcpConfig::default(),
        key: [key; 32],
    }
}

fn tuning() -> Tuning {
    let mut t = Tuning::for_profile(Profile::Balanced);
    t.keepalive = Duration::from_secs(3);
    t
}

async fn listener(key: u8) -> (KcpListener, String) {
    let l = KcpListener::bind("127.0.0.1:0", &params(key), &tuning())
        .await
        .unwrap();
    let addr = l.local_addr().unwrap().to_string();
    (l, addr)
}

async fn accept(l: &KcpListener) -> KcpStream {
    tokio::time::timeout(Duration::from_secs(5), l.accept())
        .await
        .expect("accepted in time")
        .unwrap()
        .0
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Echoes one accepted connection until the peer's FIN, then closes.
fn echo(stream: KcpStream) {
    tokio::spawn(async move {
        let (mut r, mut w) = stream.into_split();
        tokio::io::copy(&mut r, &mut w).await.unwrap();
        w.shutdown().await.unwrap();
    });
}

/// Writes `payload`, shuts down, and reads until the end of the stream.
async fn round_trip(stream: KcpStream, payload: &[u8]) -> io::Result<Vec<u8>> {
    let (mut r, mut w) = stream.into_split();
    let write = async {
        w.write_all(payload).await?;
        w.shutdown().await
    };
    let mut out = Vec::new();
    let (a, b) = tokio::join!(write, r.read_to_end(&mut out));
    a?;
    b?;
    Ok(out)
}

#[tokio::test]
async fn echo_with_half_close() {
    let (l, addr) = listener(1).await;
    let dialer = KcpDialer::new(&addr, &params(1), &tuning());
    let client = dialer.dial().await.unwrap();
    echo(accept(&l).await);
    let got = round_trip(client, b"hello over kcp").await.unwrap();
    assert_eq!(got, b"hello over kcp");
}

#[tokio::test]
async fn large_transfer_arrives_intact() {
    let (l, addr) = listener(1).await;
    let dialer = KcpDialer::new(&addr, &params(1), &tuning());
    let client = dialer.dial().await.unwrap();
    echo(accept(&l).await);
    let payload = pattern(8 << 20);
    let got = tokio::time::timeout(Duration::from_secs(60), round_trip(client, &payload))
        .await
        .expect("transfer in time")
        .unwrap();
    assert!(got == payload, "payload corrupted");
}

#[tokio::test]
async fn many_connections_on_one_listener() {
    let (l, addr) = listener(1).await;
    let l = Arc::new(l);
    let acceptor = {
        let l = l.clone();
        tokio::spawn(async move {
            loop {
                echo(accept(&l).await);
            }
        })
    };
    let dialer = Arc::new(KcpDialer::new(&addr, &params(1), &tuning()));
    let clients = (0..20).map(|i| {
        let dialer = dialer.clone();
        tokio::spawn(async move {
            let payload = pattern(1000 + i * 7919);
            let got = round_trip(dialer.dial().await.unwrap(), &payload)
                .await
                .unwrap();
            assert!(got == payload, "connection {i} corrupted");
        })
    });
    for c in clients.collect::<Vec<_>>() {
        c.await.unwrap();
    }
    acceptor.abort();
}

/// Anything not sealed with the token gets no answer and opens nothing, whether random
/// bytes or a well-formed KCP packet in the clear.
#[tokio::test]
async fn probes_get_no_answer() {
    let (l, addr) = listener(1).await;
    let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    probe.connect(&addr).await.unwrap();
    let mut kcp_open = vec![PACKET_DATA];
    kcp_open.extend_from_slice(&7u32.to_le_bytes());
    kcp_open.extend_from_slice(&[KCP_CMD_PUSH, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
    kcp_open.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, MSG_OPEN]);
    let mut probes: Vec<Vec<u8>> = vec![Vec::new(), vec![0], kcp_open, pattern(1200)];
    for len in [28, 29, 64, 1400] {
        probes.push((0..len).map(|i| (i * 7 + len) as u8).collect());
    }
    for p in &probes {
        probe.send(p).await.unwrap();
    }
    let mut buf = [0u8; 2048];
    let answer = tokio::time::timeout(Duration::from_millis(500), probe.recv(&mut buf)).await;
    assert!(answer.is_err(), "the listener answered a probe");
    let accepted = tokio::time::timeout(Duration::from_millis(100), l.accept()).await;
    assert!(accepted.is_err(), "a probe opened a connection");
}

/// A dialer with another token is ignored the same way.
#[tokio::test]
async fn wrong_key_is_ignored() {
    let (l, addr) = listener(1).await;
    let dialer = KcpDialer::new(&addr, &params(2), &tuning());
    let mut client = dialer.dial().await.unwrap();
    client.write_all(b"let me in").await.unwrap();
    let accepted = tokio::time::timeout(Duration::from_millis(500), l.accept()).await;
    assert!(accepted.is_err(), "a wrong key opened a connection");
}

/// A proxy that corrupts one byte in a fifth of the packets towards the listener: the
/// protection drops those, KCP resends them, and the data arrives intact.
#[tokio::test]
async fn tampered_packets_are_dropped() {
    let (l, addr) = listener(1).await;
    // Large buffers, so the only packets lost are the corrupted ones.
    let proxy = udp_socket("127.0.0.1:0".parse().unwrap(), 4 << 20).unwrap();
    let proxy_addr = proxy.local_addr().unwrap().to_string();
    let upstream = udp_socket("127.0.0.1:0".parse().unwrap(), 4 << 20).unwrap();
    upstream.connect(&addr).await.unwrap();
    let (proxy, upstream) = (Arc::new(proxy), Arc::new(upstream));
    let forward = {
        let (proxy, upstream) = (proxy.clone(), upstream.clone());
        tokio::spawn(async move {
            let (mut buf, mut back) = (vec![0u8; 65_536], vec![0u8; 65_536]);
            let mut client = None;
            let mut rng = 0x2545_f491_4f6c_dd1du64;
            loop {
                tokio::select! {
                    Ok((n, from)) = proxy.recv_from(&mut buf) => {
                        client = Some(from);
                        rng ^= rng << 13;
                        rng ^= rng >> 7;
                        rng ^= rng << 17;
                        if rng % 5 == 0 {
                            let i = (rng >> 8) as usize % n;
                            buf[i] ^= 0x40;
                        }
                        let _ = upstream.send(&buf[..n]).await;
                    }
                    Ok(n) = upstream.recv(&mut back) => {
                        if let Some(c) = client {
                            let _ = proxy.send_to(&back[..n], c).await;
                        }
                    }
                }
            }
        })
    };
    let dialer = KcpDialer::new(&proxy_addr, &params(1), &tuning());
    let client = dialer.dial().await.unwrap();
    echo(accept(&l).await);
    let payload = pattern(1 << 20);
    let got = tokio::time::timeout(Duration::from_secs(60), round_trip(client, &payload))
        .await
        .expect("transfer in time")
        .unwrap();
    assert!(got == payload, "payload corrupted");
    forward.abort();
}

/// A listener restarted on the same port answers the old conversation with a close, so
/// the dialer's connection fails at once instead of after the keep-alive timeout.
#[tokio::test]
async fn restarted_listener_closes_old_conversations() {
    let (l, addr) = listener(1).await;
    let dialer = KcpDialer::new(&addr, &params(1), &tuning());
    let mut client = dialer.dial().await.unwrap();
    let mut server = accept(&l).await;
    client.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).await.unwrap();
    // The dialer has heard from this listener (as after any handshake).
    server.write_all(b"pong").await.unwrap();
    client.read_exact(&mut buf).await.unwrap();
    drop((server, l));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _l = KcpListener::bind(&addr, &params(1), &tuning())
        .await
        .unwrap();
    let started = Instant::now();
    let failed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if client.write_all(b"more").await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(failed.is_ok(), "old conversation still open");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// Both halves dropped on one side: the other side reads to the end and is done.
#[tokio::test]
async fn dropping_a_stream_ends_it_cleanly() {
    let (l, addr) = listener(1).await;
    let dialer = KcpDialer::new(&addr, &params(1), &tuning());
    let mut client = dialer.dial().await.unwrap();
    let mut server = accept(&l).await;
    client.write_all(b"last words").await.unwrap();
    drop(client);
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), server.read_to_end(&mut got))
        .await
        .expect("end of stream in time")
        .unwrap();
    assert_eq!(got, b"last words");
}

/// A peer that disappears without a word (its process gone) is noticed by its silence
/// after twice the keep-alive.
#[tokio::test(flavor = "multi_thread")]
async fn silent_peer_times_out() {
    let (l, addr) = listener(1).await;
    let dialing = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dialer = KcpDialer::new(&addr, &params(1), &tuning());
            let mut client = dialer.dial().await.unwrap();
            client.write_all(b"x").await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        // Dropping the runtime stops the driver without a close, like a killed process.
    });
    let mut server = accept(&l).await;
    let mut buf = [0u8; 1];
    server.read_exact(&mut buf).await.unwrap();
    tokio::task::spawn_blocking(move || dialing.join().unwrap())
        .await
        .unwrap();
    let started = Instant::now();
    let read = tokio::time::timeout(Duration::from_secs(10), server.read(&mut buf)).await;
    let err = read
        .expect("noticed in time")
        .expect_err("silence is an error");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(!server.is_alive());
}

#[test]
fn only_a_first_data_segment_opens_a_conversation() {
    let mut first = vec![PACKET_DATA];
    first.extend_from_slice(&[9, 0, 0, 0, KCP_CMD_PUSH, 0, 0, 4]);
    first.extend_from_slice(&[0; 16]);
    assert!(opens_conversation(&first));
    let mut later = first.clone();
    later[1 + 12] = 1;
    assert!(!opens_conversation(&later));
    let mut ack = first.clone();
    ack[1 + 4] = 82;
    assert!(!opens_conversation(&ack));
    let mut ping = first.clone();
    ping[0] = PACKET_PING;
    assert!(!opens_conversation(&ping));
    assert!(!opens_conversation(&first[..10]));
}
