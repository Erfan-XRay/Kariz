use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::timeout;

use super::frame::{self, FrameType};
use super::*;

const WINDOW: u32 = 256 * 1024;

impl Transport for DuplexStream {
    type Reader = tokio::io::ReadHalf<DuplexStream>;
    type Writer = tokio::io::WriteHalf<DuplexStream>;

    fn into_halves(self) -> (Self::Reader, Self::Writer) {
        tokio::io::split(self)
    }
}

pub(super) fn config() -> SessionConfig {
    SessionConfig {
        stream_window: WINDOW,
        max_streams: 2048,
        keepalive: Duration::from_secs(5),
        coalesce: true,
        datagram_buffer: 256 * 1024,
        datagram_queue: 128,
    }
}

fn pair_with(client: SessionConfig, server: SessionConfig) -> (MuxSession, MuxSession) {
    let (a, b) = tokio::io::duplex(64 * 1024);
    (
        MuxSession::new(a, Side::Client, client),
        MuxSession::new(b, Side::Server, server),
    )
}

fn pair() -> (MuxSession, MuxSession) {
    pair_with(config(), config())
}

/// Every test step gets a deadline, so a bug fails the test instead of hanging it.
async fn within<T>(secs: u64, f: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(secs), f)
        .await
        .expect("timed out")
}

fn pattern(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_add(seed).wrapping_mul(31) % 251) as u8)
        .collect()
}

/// Accepts streams and echoes each one until the peer finishes.
fn echo_server(server: MuxSession) -> tokio::task::JoinHandle<MuxSession> {
    tokio::spawn(async move {
        while let Some((stream, _)) = server.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
        server
    })
}

async fn echo(session: &MuxSession, payload: &[u8]) -> Vec<u8> {
    let stream = session.open(Bytes::from_static(b"echo")).unwrap();
    let (mut r, mut w) = tokio::io::split(stream);
    let write = async {
        w.write_all(payload).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut got = Vec::new();
    let read = r.read_to_end(&mut got);
    let (_, res) = tokio::join!(write, read);
    res.unwrap();
    got
}

/// Waits until `f` holds, polling.
async fn eventually(mut f: impl FnMut() -> bool) {
    within(5, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

#[tokio::test]
async fn open_accept_and_half_close() {
    let (client, server) = pair();
    let mut a = client.open(Bytes::from_static(b"example.com:443")).unwrap();
    a.write_all(b"hello").await.unwrap();
    a.shutdown().await.unwrap();

    let (mut b, syn) = within(5, server.accept()).await.unwrap();
    assert_eq!(&syn[..], b"example.com:443");
    let mut got = Vec::new();
    within(5, b.read_to_end(&mut got)).await.unwrap();
    assert_eq!(got, b"hello");

    // The other direction is still open after the half close.
    b.write_all(b"world").await.unwrap();
    b.shutdown().await.unwrap();
    let mut got = Vec::new();
    within(5, a.read_to_end(&mut got)).await.unwrap();
    assert_eq!(got, b"world");

    drop((a, b));
    eventually(|| client.stream_count() == 0 && server.stream_count() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn thousand_parallel_streams() {
    let (client, server) = pair();
    let _server = echo_server(server);
    let client = std::sync::Arc::new(client);
    let mut tasks = Vec::new();
    for i in 0..1000 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let payload = pattern(100 + i * 37 % 5000, i);
            assert_eq!(echo(&client, &payload).await, payload, "stream {i}");
        }));
    }
    within(60, async {
        for t in tasks {
            t.await.unwrap();
        }
    })
    .await;
    eventually(|| client.stream_count() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn large_transfers_with_and_without_coalescing() {
    for coalesce in [true, false] {
        let cfg = SessionConfig {
            coalesce,
            ..config()
        };
        let (client, server) = pair_with(cfg.clone(), cfg);
        let _server = echo_server(server);
        let payload = pattern(8 * 1024 * 1024, 7);
        let got = within(60, echo(&client, &payload)).await;
        assert!(got == payload, "corrupted (coalesce = {coalesce})");
    }
}

#[tokio::test]
async fn reset_reaches_the_peer() {
    let (client, server) = pair();

    // Explicit reset: the exit side could not reach the target.
    let mut a = client.open(Bytes::from_static(b"dead:1")).unwrap();
    let (b, _) = within(5, server.accept()).await.unwrap();
    b.reset(ResetReason::DialFailed);
    let mut buf = [0u8; 8];
    let err = within(5, a.read(&mut buf)).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
    assert!(a.write_all(b"x").await.is_err());

    // Dropping a stream before it is finished resets it too.
    let mut a = client.open(Bytes::from_static(b"t")).unwrap();
    let (b, _) = within(5, server.accept()).await.unwrap();
    drop(b);
    let err = within(5, a.read(&mut buf)).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);

    // Streams can be written to right after open, before the peer even accepted.
    let mut a = client.open(Bytes::from_static(b"t")).unwrap();
    a.write_all(b"early").await.unwrap();
    let (mut b, _) = within(5, server.accept()).await.unwrap();
    let mut got = [0u8; 5];
    within(5, b.read_exact(&mut got)).await.unwrap();
    assert_eq!(&got, b"early");
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_api_carries_data_and_end_of_stream() {
    let (client, server) = pair();
    let a = client.open(Bytes::from_static(b"t")).unwrap();
    let data = Bytes::from(pattern(3 * 1024 * 1024, 5));
    let d = data.clone();
    // Far more than one window: `send` must wait for credit, not fail or overrun.
    let sender = tokio::spawn(async move {
        a.send(d).await.unwrap();
        a.finish().unwrap();
        a
    });
    let (b, _) = within(5, server.accept()).await.unwrap();
    let mut got = Vec::new();
    within(30, async {
        while let Some(chunk) = b.recv().await.unwrap() {
            got.extend_from_slice(&chunk);
        }
    })
    .await;
    assert!(got == data);
    let a = sender.await.unwrap();
    // The other direction still works after the half close.
    b.send(Bytes::from_static(b"reply")).await.unwrap();
    b.finish().unwrap();
    assert_eq!(&within(5, a.recv()).await.unwrap().unwrap()[..], b"reply");
    assert!(within(5, a.recv()).await.unwrap().is_none());
}

/// Writes to `stream` until a write blocks for `patience`; returns the bytes accepted.
async fn fill(stream: &mut MuxStream, patience: Duration) -> usize {
    let chunk = [0x55u8; 4096];
    let mut total = 0;
    loop {
        match timeout(patience, stream.write(&chunk)).await {
            Ok(Ok(n)) => total += n,
            Ok(Err(e)) => panic!("write failed: {e}"),
            Err(_) => return total,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn window_accounting_is_exact() {
    let (client, server) = pair();
    let mut a = client.open(Bytes::from_static(b"t")).unwrap();
    let (mut b, _) = within(5, server.accept()).await.unwrap();

    // The peer never reads: exactly one window fits.
    let patience = Duration::from_millis(500);
    assert_eq!(fill(&mut a, patience).await, WINDOW as usize);

    // Reading half a window hands exactly that much credit back.
    let mut sink = vec![0u8; WINDOW as usize / 2];
    within(5, b.read_exact(&mut sink)).await.unwrap();
    assert_eq!(fill(&mut a, patience).await, WINDOW as usize / 2);

    // Reading less than half a window is not worth a WINDOW frame yet.
    let mut sink = vec![0u8; WINDOW as usize / 4];
    within(5, b.read_exact(&mut sink)).await.unwrap();
    assert_eq!(fill(&mut a, patience).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_reader_does_not_block_other_streams() {
    let (client, server) = pair();
    let mut stuck = client.open(Bytes::from_static(b"slow")).unwrap();
    let (_stuck_peer, _) = within(5, server.accept()).await.unwrap();
    let _server = echo_server(server);

    // Fill the slow stream's window; it now blocks...
    assert!(fill(&mut stuck, Duration::from_millis(300)).await > 0);
    // ...while another stream on the same session moves megabytes.
    let payload = pattern(4 * 1024 * 1024, 3);
    assert!(within(30, echo(&client, &payload)).await == payload);
}

#[tokio::test]
async fn dead_peer_is_detected_by_ping() {
    let cfg = SessionConfig {
        keepalive: Duration::from_millis(100),
        ..config()
    };
    // The other end of the pipe stays open but never answers.
    let (a, _silent): (DuplexStream, DuplexStream) = tokio::io::duplex(64 * 1024);
    let client = MuxSession::new(a, Side::Client, cfg);
    let mut stream = client.open(Bytes::from_static(b"t")).unwrap();
    within(2, client.closed()).await;
    let mut buf = [0u8; 1];
    let err = stream.read(&mut buf).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
    assert!(err.to_string().contains("ping"), "{err}");
    assert!(client.open(Bytes::new()).is_err());
}

#[tokio::test]
async fn idle_session_stays_up() {
    let cfg = SessionConfig {
        keepalive: Duration::from_millis(50),
        ..config()
    };
    let (client, server) = pair_with(cfg.clone(), cfg);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!client.is_closed() && !server.is_closed());
    let _server = echo_server(server);
    assert_eq!(within(5, echo(&client, b"still here")).await, b"still here");
}

#[tokio::test]
async fn goaway_stops_new_streams_only() {
    let (client, server) = pair();
    let mut a = client.open(Bytes::from_static(b"t")).unwrap();
    let (mut b, _) = within(5, server.accept()).await.unwrap();

    client.goaway();
    assert!(client.open(Bytes::new()).is_err());
    eventually(|| server.is_draining()).await;
    assert!(server.open(Bytes::new()).is_err());
    assert!(within(5, server.accept()).await.is_none());

    // The existing stream keeps working.
    a.write_all(b"ping").await.unwrap();
    let mut got = [0u8; 4];
    within(5, b.read_exact(&mut got)).await.unwrap();
    b.write_all(b"pong").await.unwrap();
    within(5, a.read_exact(&mut got)).await.unwrap();
    assert_eq!(&got, b"pong");
    assert!(!client.is_closed());
}

#[tokio::test]
async fn streams_over_the_peer_limit_are_refused() {
    let server_cfg = SessionConfig {
        max_streams: 2,
        ..config()
    };
    let (client, server) = pair_with(config(), server_cfg);
    let mut streams: Vec<MuxStream> = (0..3)
        .map(|_| client.open(Bytes::from_static(b"t")).unwrap())
        .collect();
    let _held = (
        within(5, server.accept()).await.unwrap(),
        within(5, server.accept()).await.unwrap(),
    );
    let mut buf = [0u8; 1];
    let err = within(5, streams[2].read(&mut buf)).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
    assert!(err.to_string().contains("refused"), "{err}");
    assert!(!client.is_closed());
}

/// A raw peer that writes hand-made frames.
async fn raw_peer(frames: Vec<u8>) -> MuxSession {
    let (mut raw, b) = tokio::io::duplex(1 << 20);
    let server = MuxSession::new(b, Side::Server, config());
    raw.write_all(&frames).await.unwrap();
    // Keep the raw end open so only the frames can end the session.
    tokio::spawn(async move {
        let mut sink = vec![0u8; 4096];
        while raw.read(&mut sink).await.is_ok_and(|n| n > 0) {}
    });
    server
}

#[tokio::test]
async fn protocol_violations_close_the_session() {
    // More data than the window allows (the acceptor grants the full window on SYN).
    let mut frames = Vec::new();
    frame::put(&mut frames, FrameType::Syn, 1, b"t");
    let chunk = vec![0u8; MAX_DATA_FRAME];
    for _ in 0..(WINDOW as usize / MAX_DATA_FRAME + 1) {
        frame::put(&mut frames, FrameType::Data, 1, &chunk);
    }
    let server = raw_peer(frames).await;
    within(2, server.closed()).await;

    // A client opening an even (server-side) stream id.
    let mut frames = Vec::new();
    frame::put(&mut frames, FrameType::Syn, 2, b"t");
    within(2, raw_peer(frames).await.closed()).await;

    // Reusing an old stream id.
    let mut frames = Vec::new();
    frame::put(&mut frames, FrameType::Syn, 5, b"t");
    frame::put(&mut frames, FrameType::Syn, 3, b"t");
    within(2, raw_peer(frames).await.closed()).await;

    // Unknown frame type.
    within(2, raw_peer(vec![42, 0, 0, 0, 1, 0, 0]).await.closed()).await;
}

#[tokio::test]
async fn closing_the_connection_ends_every_stream() {
    let (client, server) = pair();
    let mut a = client.open(Bytes::from_static(b"t")).unwrap();
    let _b = within(5, server.accept()).await.unwrap();
    drop(server);
    within(2, client.closed()).await;
    let mut buf = [0u8; 1];
    assert!(a.read(&mut buf).await.is_err());
    assert!(a.write_all(b"x").await.is_err());
}

/// Small deterministic PRNG, so a failing seed can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

/// Many streams with random sizes, write chunking, close order and resets, over tiny
/// windows. Data on streams that were not reset must arrive intact, resets must reach
/// the peer as errors, and no stream may be left behind on either side.
#[tokio::test(flavor = "multi_thread")]
async fn randomized_stress() {
    for seed in 1..=4u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let cfg = SessionConfig {
            stream_window: 16 * 1024,
            coalesce: rng.below(2) == 0,
            ..config()
        };
        let (client, server) = pair_with(cfg.clone(), cfg);
        let client = std::sync::Arc::new(client);
        let server = std::sync::Arc::new(server);

        // Server: echoes in random chunk sizes; some streams it resets after a while.
        let srv = server.clone();
        let server_task = tokio::spawn(async move {
            while let Some((mut stream, syn)) = srv.accept().await {
                let plan = u64::from_be_bytes(syn[..8].try_into().unwrap());
                tokio::spawn(async move {
                    let mut rng = Rng(plan | 1);
                    let reset_after = (plan % 5 == 0).then(|| rng.below(50_000));
                    let mut buf = vec![0u8; 1 + rng.below(40_000)];
                    let mut seen = 0;
                    loop {
                        let n = match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        seen += n;
                        if reset_after.is_some_and(|r| seen > r) {
                            stream.reset(ResetReason::Cancel);
                            return;
                        }
                        if stream.write_all(&buf[..n]).await.is_err() {
                            return;
                        }
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });

        let mut tasks = Vec::new();
        for i in 0..200u64 {
            let plan = rng.next() & !0xff | i;
            let len = rng.below(200_000);
            let chunk = 1 + rng.below(30_000);
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                let payload = pattern(len, plan as usize);
                let stream = client
                    .open(Bytes::copy_from_slice(&plan.to_be_bytes()))
                    .unwrap();
                let (mut r, mut w) = tokio::io::split(stream);
                let write = async {
                    for c in payload.chunks(chunk) {
                        w.write_all(c).await?;
                    }
                    w.shutdown().await
                };
                let mut got = Vec::new();
                let (wres, rres) = tokio::join!(write, r.read_to_end(&mut got));
                let reset_planned = plan % 5 == 0;
                match (wres, rres) {
                    (Ok(()), Ok(_)) => {
                        assert!(got == payload, "seed {seed} stream {i}: corrupted");
                        false
                    }
                    // A reset shows up as an error on at least one side of the stream.
                    _ if reset_planned => true,
                    (w, r) => panic!("seed {seed} stream {i}: unexpected {w:?} {r:?}"),
                }
            }));
        }
        let resets = within(120, async {
            let mut resets = 0;
            for t in tasks {
                resets += t.await.unwrap() as usize;
            }
            resets
        })
        .await;
        // Both paths must actually be exercised.
        assert!((10..190).contains(&resets), "seed {seed}: {resets} resets");
        eventually(|| client.stream_count() == 0 && server.stream_count() == 0).await;
        assert!(!client.is_closed() && !server.is_closed());
        client.close();
        server_task.await.unwrap();
    }
}

/// Single-stream throughput over localhost TCP, raw vs. through the mux:
/// `cargo test --release --lib mux_throughput -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn mux_throughput() {
    use tokio::net::{TcpListener, TcpStream};

    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).await.unwrap();
        let (s, _) = l.accept().await.unwrap();
        c.set_nodelay(true).unwrap();
        s.set_nodelay(true).unwrap();
        (c, s)
    }

    async fn run<S>(stream: S, payload: &[u8]) -> f64
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (mut r, mut w) = tokio::io::split(stream);
        let start = std::time::Instant::now();
        let write = async {
            for c in payload.chunks(64 * 1024) {
                w.write_all(c).await.unwrap();
            }
            w.shutdown().await.unwrap();
        };
        let mut got = Vec::with_capacity(payload.len());
        let (_, res) = tokio::join!(write, r.read_to_end(&mut got));
        res.unwrap();
        assert_eq!(got.len(), payload.len());
        payload.len() as f64 * 8.0 / start.elapsed().as_secs_f64() / 1e6
    }

    fn echo<S>(stream: S) -> tokio::task::JoinHandle<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(stream);
            tokio::io::copy_buf(
                &mut tokio::io::BufReader::with_capacity(64 * 1024, &mut r),
                &mut w,
            )
            .await
            .unwrap();
            w.shutdown().await.unwrap();
        })
    }

    fn secure<S: tokio::io::AsyncRead + tokio::io::AsyncWrite>(
        s: S,
        client: bool,
    ) -> crate::crypto::record::SecureStream<S> {
        use crate::crypto::{record::*, Cipher};
        let (k1, k2) = ([1u8; 32], [2u8; 32]);
        let (send, recv) = if client { (k1, k2) } else { (k2, k1) };
        SecureStream::new(
            s,
            Sealer::new(Cipher::Aes256Gcm, &send).unwrap(),
            Opener::new(Cipher::Aes256Gcm, &recv).unwrap(),
            None,
        )
    }

    let mb = std::env::var("MUX_BENCH_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let window: u32 = std::env::var("MUX_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(WINDOW);
    let cfg = SessionConfig {
        stream_window: window,
        ..config()
    };
    let payload = pattern(mb * 1024 * 1024, 1);
    for encrypted in [false, true] {
        for _ in 0..2 {
            let (c, s) = tcp_pair().await;
            let direct = if encrypted {
                let _e = echo(secure(s, false));
                run(secure(c, true), &payload).await
            } else {
                let _e = echo(s);
                run(c, &payload).await
            };

            let (c, s) = tcp_pair().await;
            // Sessions run over independent halves, as in the tunnel.
            let (client, server) = if encrypted {
                let (cr, cw) = secure(c, true).split(TcpStream::into_split);
                let (sr, sw) = secure(s, false).split(TcpStream::into_split);
                (
                    MuxSession::from_halves(cr, cw, Side::Client, cfg.clone()),
                    MuxSession::from_halves(sr, sw, Side::Server, cfg.clone()),
                )
            } else {
                let (cr, cw) = c.into_split();
                let (sr, sw) = s.into_split();
                (
                    MuxSession::from_halves(cr, cw, Side::Client, cfg.clone()),
                    MuxSession::from_halves(sr, sw, Side::Server, cfg.clone()),
                )
            };
            let stream = client.open(Bytes::new()).unwrap();
            let (peer, _) = server.accept().await.unwrap();
            let _e = echo(peer);
            let mux = run(stream, &payload).await;
            let label = if encrypted { "encrypted" } else { "plain    " };
            println!(
                "{label}: without mux {direct:.0} Mbit/s, with mux {mux:.0} Mbit/s ({:.0} %)",
                mux / direct * 100.0
            );
        }
    }
}

#[test]
fn reset_reasons_roundtrip() {
    use super::ResetReason as R;
    for r in [
        R::Cancel,
        R::DialFailed,
        R::Refused,
        R::Protocol,
        R::Unsupported,
    ] {
        assert_eq!(R::from_id(r.id()), r);
    }
    // Unknown reasons from newer peers read as a plain reset.
    assert_eq!(R::from_id(200), R::Cancel);
    assert_eq!(R::Unsupported.to_error().kind(), io::ErrorKind::Unsupported);
}

// ---- Datagrams (phase 3) ----

/// Records what the session writes, but accepts nothing until [`Gate::open`], so a test
/// can queue frames while the writer is stuck and then check the order they leave in.
#[derive(Clone, Default)]
struct Gate(std::sync::Arc<std::sync::Mutex<GateState>>);

#[derive(Default)]
struct GateState {
    open: bool,
    waiting: Option<std::task::Waker>,
    written: Vec<u8>,
}

impl Gate {
    fn open(&self) {
        let mut g = self.0.lock().unwrap();
        g.open = true;
        if let Some(w) = g.waiting.take() {
            w.wake();
        }
    }

    /// The writer is blocked on the closed gate.
    fn blocked(&self) -> bool {
        self.0.lock().unwrap().waiting.is_some()
    }

    fn frames(&self) -> Vec<(FrameType, u32, usize)> {
        let g = self.0.lock().unwrap();
        let mut out = Vec::new();
        let mut rest = &g.written[..];
        while rest.len() >= frame::HEADER_LEN {
            let h = frame::Header::decode(rest[..frame::HEADER_LEN].try_into().unwrap()).unwrap();
            out.push((h.kind, h.stream, h.len as usize));
            rest = &rest[frame::HEADER_LEN + h.len as usize..];
        }
        out
    }
}

impl tokio::io::AsyncWrite for Gate {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let mut g = self.0.lock().unwrap();
        if !g.open {
            g.waiting = Some(cx.waker().clone());
            return std::task::Poll::Pending;
        }
        g.written.extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// A client session writing into a closed [`Gate`], with a stream `a` already opened
/// (its `SYN` is the batch stuck at the gate). The duplex end keeps the reader open.
async fn gated(cfg: SessionConfig) -> (Gate, MuxSession, MuxStream, DuplexStream) {
    let gate = Gate::default();
    let (read_side, keep) = tokio::io::duplex(1024);
    let session = MuxSession::from_halves(read_side, gate.clone(), Side::Client, cfg);
    let a = session.open(Bytes::from_static(b"a")).unwrap();
    eventually(|| gate.blocked()).await;
    (gate, session, a, keep)
}

/// Kinds of the `DATA` / `DGRAM` frames, in wire order, after the first write.
fn payload_kinds(frames: &[(FrameType, u32, usize)]) -> Vec<FrameType> {
    frames
        .iter()
        .map(|f| f.0)
        .filter(|k| matches!(k, FrameType::Data | FrameType::Dgram))
        .collect()
}

#[tokio::test]
async fn datagrams_keep_boundaries_and_order() {
    for coalesce in [true, false] {
        let cfg = SessionConfig {
            coalesce,
            ..config()
        };
        let (client, server) = pair_with(cfg.clone(), cfg);
        let echo = tokio::spawn(async move {
            let (stream, syn) = server.accept().await.unwrap();
            assert_eq!(&syn[..], b"udp");
            while let Some(p) = stream.recv_datagram().await.unwrap() {
                assert!(stream.send_datagram(p));
            }
            stream.finish().unwrap();
            server
        });
        // Sent right after the open: the SYN must still arrive first.
        let stream = client.open(Bytes::from_static(b"udp")).unwrap();
        let packets: Vec<Vec<u8>> = [0, 1, 1400, 9000, 65_535]
            .iter()
            .enumerate()
            .map(|(i, &len)| pattern(len, i))
            .collect();
        for p in &packets {
            assert!(stream.send_datagram(Bytes::from(p.clone())));
        }
        for p in &packets {
            let got = within(5, stream.recv_datagram()).await.unwrap().unwrap();
            assert_eq!(&got[..], &p[..], "coalesce {coalesce}");
        }
        stream.finish().unwrap();
        assert!(within(5, stream.recv_datagram()).await.unwrap().is_none());
        assert!(!stream.send_datagram(Bytes::from_static(b"late")));
        let server = within(5, echo).await.unwrap();
        assert_eq!(server.datagram_stats(), DatagramStats::default());
    }
}

#[tokio::test]
async fn datagrams_overtake_queued_stream_data() {
    let (gate, session, a, _keep) = gated(config()).await;
    // Stream data queued first (within the initial credit), then a datagram.
    within(5, a.send(Bytes::from(vec![1u8; 64_000])))
        .await
        .unwrap();
    let b = session.open(Bytes::from_static(b"b")).unwrap();
    assert!(b.send_datagram(Bytes::from_static(b"ping")));
    gate.open();
    eventually(|| payload_kinds(&gate.frames()).len() >= 5).await;
    let frames = gate.frames();
    let kinds = payload_kinds(&frames);
    assert_eq!(kinds[0], FrameType::Dgram, "{frames:?}");
    // b's SYN went out before its datagram.
    let syn_b = frames
        .iter()
        .position(|f| f.0 == FrameType::Syn && f.1 == b.id());
    let dgram = frames.iter().position(|f| f.0 == FrameType::Dgram);
    assert!(syn_b.unwrap() < dgram.unwrap());
}

#[tokio::test]
async fn datagram_floods_do_not_starve_streams() {
    for coalesce in [true, false] {
        let cfg = SessionConfig {
            coalesce,
            ..config()
        };
        let (gate, session, a, _keep) = gated(cfg).await;
        within(5, a.send(Bytes::from(vec![1u8; 64_000])))
            .await
            .unwrap();
        let b = session.open(Bytes::from_static(b"b")).unwrap();
        // More datagrams than one batch holds.
        for _ in 0..100 {
            assert!(b.send_datagram(Bytes::from(vec![2u8; 1400])));
        }
        gate.open();
        eventually(|| payload_kinds(&gate.frames()).len() >= 104).await;
        let kinds = payload_kinds(&gate.frames());
        let first_data = kinds.iter().position(|k| *k == FrameType::Data).unwrap();
        let last_dgram = kinds.iter().rposition(|k| *k == FrameType::Dgram).unwrap();
        assert!(first_data < last_dgram, "coalesce {coalesce}: {kinds:?}");
        if !coalesce {
            // One frame per write, taking turns.
            assert_eq!(
                &kinds[..4],
                &[
                    FrameType::Dgram,
                    FrameType::Data,
                    FrameType::Dgram,
                    FrameType::Data
                ]
            );
        }
    }
}

#[tokio::test]
async fn full_send_queue_drops_instead_of_blocking() {
    let cfg = SessionConfig {
        datagram_buffer: 10_000,
        ..config()
    };
    let (_gate, session, a, _keep) = gated(cfg).await;
    let accepted = (0..20)
        .filter(|_| a.send_datagram(Bytes::from(vec![0u8; 1400])))
        .count();
    // 1,407 bytes each with the frame header.
    assert_eq!(accepted, 10_000 / 1407);
    assert_eq!(
        session.datagram_stats().dropped_send,
        (20 - accepted) as u64
    );
}

#[tokio::test]
async fn full_receive_queue_drops_the_oldest() {
    let (client, server) = pair();
    let stream = client.open(Bytes::from_static(b"udp")).unwrap();
    for i in 0..200u32 {
        assert!(stream.send_datagram(Bytes::from(i.to_be_bytes().to_vec())));
    }
    let (peer, _) = within(5, server.accept()).await.unwrap();
    eventually(|| server.datagram_stats().dropped_recv == 72).await;
    // The newest 128 are left, in order.
    for i in 72..200u32 {
        let p = within(5, peer.recv_datagram()).await.unwrap().unwrap();
        assert_eq!(&p[..], &i.to_be_bytes());
    }
}

#[tokio::test]
async fn datagrams_use_no_stream_credit() {
    let (client, server) = pair();
    let stream = client.open(Bytes::from_static(b"udp")).unwrap();
    let (peer, _) = within(5, server.accept()).await.unwrap();
    // Far more than the stream window, with nobody reading stream data.
    let total = WINDOW as usize * 8;
    let sender = tokio::spawn(async move {
        let mut sent = 0;
        while sent < total {
            if stream.send_datagram(Bytes::from(vec![3u8; 60_000])) {
                sent += 60_000;
            } else {
                tokio::task::yield_now().await;
            }
        }
        stream
    });
    let mut received = 0;
    while received < total {
        received += within(5, peer.recv_datagram())
            .await
            .unwrap()
            .unwrap()
            .len();
    }
    let stream = sender.await.unwrap();
    // Stream data still flows with its own, untouched credit.
    within(5, stream.send(Bytes::from(vec![4u8; 1000])))
        .await
        .unwrap();
    let got = within(5, peer.recv()).await.unwrap().unwrap();
    assert_eq!(got.len(), 1000);
}

#[tokio::test]
async fn stray_datagrams_are_ignored() {
    let mut frames = Vec::new();
    frame::put(&mut frames, FrameType::Dgram, 7, b"unknown stream");
    frame::put(&mut frames, FrameType::Syn, 1, b"udp");
    frame::put(&mut frames, FrameType::Dgram, 1, b"hello");
    frame::put(&mut frames, FrameType::Rst, 1, &[0]);
    frame::put(&mut frames, FrameType::Dgram, 1, b"after reset");
    let server = raw_peer(frames).await;
    let (stream, _) = within(2, server.accept()).await.unwrap();
    assert_eq!(
        &within(2, stream.recv_datagram()).await.unwrap().unwrap()[..],
        b"hello"
    );
    assert!(within(2, stream.recv_datagram()).await.is_err(), "reset");
    assert!(!server.is_closed());
}
