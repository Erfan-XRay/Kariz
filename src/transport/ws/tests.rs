//! WebSocket stream and upgrade tests: codec behaviour over in-memory pipes, and the
//! listener's answers to probes over real sockets.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{
    duplex, split, AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadBuf, ReadHalf,
    WriteHalf,
};
use tokio::net::TcpStream;

use super::frame::*;
use super::*;
use crate::config::{TransportKind, Tuning, WsConfig};
use crate::transport::{Dialer, Listener, Settings};

type Pipe = WsStream<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

fn pair() -> (Pipe, Pipe) {
    let (a, b) = duplex(1 << 20);
    let (ar, aw) = split(a);
    let (br, bw) = split(b);
    (
        WsStream::new(ar, aw, Role::Client, &[]).unwrap(),
        WsStream::new(br, bw, Role::Server, &[]).unwrap(),
    )
}

/// A client or server reading `raw` bytes crafted by the test, and the other end of its
/// connection (to see what it sends back).
fn reader_of(
    raw: Vec<u8>,
    role: Role,
) -> (WsStream<Trickle, WriteHalf<DuplexStream>>, DuplexStream) {
    let (ours, theirs) = duplex(1 << 16);
    let (_, w) = split(ours);
    let stream = WsStream::new(Trickle::new(raw, usize::MAX), w, role, &[]).unwrap();
    (stream, theirs)
}

/// Encodes one frame as a peer would.
fn raw(fin: bool, opcode: u8, mask: Option<[u8; 4]>, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    Header {
        fin,
        opcode,
        mask,
        len: payload.len() as u64,
    }
    .encode(&mut out);
    let start = out.len();
    out.extend_from_slice(payload);
    if let Some(key) = mask {
        apply_mask(&mut out[start..], key, 0);
    }
    out
}

/// Serves fixed bytes at most `step` at a time, then end of stream.
struct Trickle {
    data: Vec<u8>,
    pos: usize,
    step: usize,
}

impl Trickle {
    fn new(data: Vec<u8>, step: usize) -> Self {
        Self { data, pos: 0, step }
    }
}

impl AsyncRead for Trickle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = (self.data.len() - self.pos)
            .min(self.step)
            .min(buf.remaining());
        let pos = self.pos;
        buf.put_slice(&self.data[pos..pos + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

async fn read_all<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    r.read_to_end(&mut out).await?;
    Ok(out)
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 131 % 251) as u8).collect()
}

#[tokio::test]
async fn roundtrip_in_both_directions() {
    let (client, server) = pair();
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    // Sizes cover 7-bit, 16-bit and 64-bit frame lengths and writes split into frames.
    let sizes = [
        0usize,
        1,
        125,
        126,
        65_535,
        65_536,
        MAX_FRAME_OUT + 1,
        1_000_000,
    ];
    let payload: Vec<u8> = pattern(sizes.iter().sum());
    let expected = payload.clone();
    let up = tokio::spawn(async move {
        let mut pos = 0;
        for n in sizes {
            cw.write_all(&payload[pos..pos + n]).await.unwrap();
            pos += n;
        }
        cw.shutdown().await.unwrap();
    });
    // The server echoes everything back, then closes its side.
    let echo = tokio::spawn(async move {
        let got = read_all(&mut sr).await.unwrap();
        sw.write_all(&got).await.unwrap();
        sw.shutdown().await.unwrap();
        got
    });
    let back = read_all(&mut cr).await.unwrap();
    up.await.unwrap();
    assert!(
        echo.await.unwrap() == expected,
        "client to server corrupted"
    );
    assert!(back == expected, "server to client corrupted");
}

#[tokio::test]
async fn vectored_writes_become_one_frame() {
    let (a, mut wire) = duplex(1 << 16);
    let (_, w) = split(a);
    let (_, mut writer) = WsStream::new(tokio::io::empty(), w, Role::Server, &[])
        .unwrap()
        .into_split();
    let parts = [io::IoSlice::new(b"abc"), io::IoSlice::new(b"defg")];
    assert_eq!(writer.write_vectored(&parts).await.unwrap(), 7);
    writer.flush().await.unwrap();
    drop(writer);
    let bytes = read_all(&mut wire).await.unwrap();
    assert_eq!(bytes, raw(true, OP_BINARY, None, b"abcdefg"));
}

#[tokio::test]
async fn client_masks_and_server_does_not() {
    let secret = b"a fairly recognisable plaintext";
    for role in [Role::Client, Role::Server] {
        let (a, mut wire) = duplex(1 << 16);
        let (_, w) = split(a);
        let (_, mut writer) = WsStream::new(tokio::io::empty(), w, role, &[])
            .unwrap()
            .into_split();
        writer.write_all(secret).await.unwrap();
        writer.write_all(secret).await.unwrap();
        writer.flush().await.unwrap();
        drop(writer);
        let bytes = read_all(&mut wire).await.unwrap();
        let (h, n) = Header::decode(&bytes).unwrap().unwrap();
        assert_eq!(
            (h.fin, h.opcode, h.len),
            (true, OP_BINARY, secret.len() as u64)
        );
        let visible = bytes.windows(secret.len()).any(|w| w == secret);
        match role {
            Role::Client => {
                assert!(h.mask.is_some());
                assert!(!visible, "masked payload shows the plaintext");
                // A fresh key per frame.
                let (h2, _) = Header::decode(&bytes[n + secret.len()..]).unwrap().unwrap();
                assert_ne!(h.mask, h2.mask);
            }
            Role::Server => {
                assert!(h.mask.is_none());
                assert!(visible);
            }
        }
    }
}

#[tokio::test]
async fn fragments_and_interleaved_control_frames() {
    // RFC 6455 section 5.4: control frames may appear between fragments.
    let mask = Some([1, 2, 3, 4]);
    let mut wire = raw(false, OP_BINARY, mask, b"Hel");
    wire.extend(raw(true, OP_PING, mask, b"are you there"));
    wire.extend(raw(false, OP_CONTINUATION, mask, b""));
    wire.extend(raw(true, OP_PONG, mask, b"unsolicited"));
    wire.extend(raw(true, OP_CONTINUATION, mask, b"lo"));
    wire.extend(raw(true, OP_BINARY, mask, b", world"));
    wire.extend(raw(true, OP_CLOSE, mask, &1000u16.to_be_bytes()));
    wire.extend(raw(true, OP_BINARY, mask, b"after close: ignored"));

    // Every split of the input gives the same result.
    for step in [1, 2, 3, 7, 64, usize::MAX] {
        let (ours, mut theirs) = duplex(1 << 16);
        let (_, w) = split(ours);
        let (mut r, mut w) = WsStream::new(Trickle::new(wire.clone(), step), w, Role::Server, &[])
            .unwrap()
            .into_split();
        assert_eq!(
            read_all(&mut r).await.unwrap(),
            b"Hello, world",
            "step {step}"
        );

        // The ping is answered on the next flush, unmasked (we are the server).
        w.flush().await.unwrap();
        w.shutdown().await.unwrap();
        drop(w);
        let sent = read_all(&mut theirs).await.unwrap();
        let mut expected = raw(true, OP_PONG, None, b"are you there");
        expected.extend(raw(true, OP_CLOSE, None, &1000u16.to_be_bytes()));
        assert_eq!(sent, expected, "step {step}");
    }
}

#[tokio::test]
async fn pong_goes_before_the_next_data_frame() {
    let mut wire = raw(true, OP_PING, None, b"1");
    wire.extend(raw(true, OP_PING, None, b"2"));
    wire.extend(raw(true, OP_BINARY, None, b"data"));
    let (stream, mut theirs) = reader_of(wire, Role::Client);
    let (mut r, mut w) = stream.into_split();
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).await.unwrap();
    w.write_all(b"reply").await.unwrap();
    w.flush().await.unwrap();
    drop(w);
    let sent = read_all(&mut theirs).await.unwrap();
    // Only the latest ping is answered.
    let (h, n) = Header::decode(&sent).unwrap().unwrap();
    assert_eq!((h.opcode, h.len), (OP_PONG, 1));
    let mut payload = sent[n..n + 1].to_vec();
    apply_mask(&mut payload, h.mask.unwrap(), 0);
    assert_eq!(payload, b"2");
    let (h, _) = Header::decode(&sent[n + 1..]).unwrap().unwrap();
    assert_eq!((h.opcode, h.len), (OP_BINARY, 5));
}

#[tokio::test]
async fn protocol_violations_are_errors() {
    let m = Some([9, 8, 7, 6]);
    let cases: Vec<(&str, Role, Vec<u8>)> = vec![
        ("text frame", Role::Server, raw(true, OP_TEXT, m, b"hi")),
        (
            "unmasked frame to server",
            Role::Server,
            raw(true, OP_BINARY, None, b"hi"),
        ),
        (
            "unmasked ping to server",
            Role::Server,
            raw(true, OP_PING, None, b""),
        ),
        (
            "masked frame to client",
            Role::Client,
            raw(true, OP_BINARY, m, b"hi"),
        ),
        (
            "continuation first",
            Role::Server,
            raw(true, OP_CONTINUATION, m, b"hi"),
        ),
        (
            "new message inside a fragmented one",
            Role::Server,
            [
                raw(false, OP_BINARY, m, b"a"),
                raw(true, OP_BINARY, m, b"b"),
            ]
            .concat(),
        ),
        (
            "reserved opcode",
            Role::Server,
            vec![0x83, 0x80, 0, 0, 0, 0],
        ),
        ("frame too large", Role::Client, {
            let mut v = Vec::new();
            Header {
                fin: true,
                opcode: OP_BINARY,
                mask: None,
                len: MAX_FRAME_IN + 1,
            }
            .encode(&mut v);
            v
        }),
    ];
    for (name, role, wire) in cases {
        let (mut stream, _peer) = reader_of(wire, role);
        let err = read_all(&mut stream).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{name}: {err}");
    }
}

#[tokio::test]
async fn truncation_is_an_error_but_a_clean_end_is_eof() {
    let frame = raw(true, OP_BINARY, None, b"0123456789");
    for cut in 1..frame.len() {
        let (mut stream, _peer) = reader_of(frame[..cut].to_vec(), Role::Client);
        let err = read_all(&mut stream).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut {cut}");
    }
    let unfinished = raw(false, OP_BINARY, None, b"part");
    let (mut stream, _peer) = reader_of(unfinished, Role::Client);
    assert!(read_all(&mut stream).await.is_err());

    // A peer that just closes the connection between messages.
    let (mut stream, _peer) = reader_of(frame, Role::Client);
    assert_eq!(read_all(&mut stream).await.unwrap(), b"0123456789");
}

#[tokio::test]
async fn half_close_keeps_the_other_direction() {
    let (client, server) = pair();
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    cw.write_all(b"request").await.unwrap();
    cw.shutdown().await.unwrap();
    assert!(
        cw.write_all(b"more").await.is_err(),
        "no data after a close frame"
    );
    assert_eq!(read_all(&mut sr).await.unwrap(), b"request");
    sw.write_all(b"response").await.unwrap();
    sw.shutdown().await.unwrap();
    assert_eq!(read_all(&mut cr).await.unwrap(), b"response");
}

#[tokio::test]
async fn upgrade_keeps_bytes_that_follow_the_head() {
    let (mut client_io, mut server_io) = duplex(1 << 16);
    let config = ClientConfig::new(None, "127.0.0.1:80", "http", 80);
    let server = tokio::spawn(async move {
        // A server that sends its first frame in the same write as the 101.
        let mut buf = vec![0u8; 4096];
        let mut got = Vec::new();
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = server_io.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8(got).unwrap();
        let key = text
            .lines()
            .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
            .unwrap();
        let mut answer = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            upgrade::accept_key(key)
        )
        .into_bytes();
        answer.extend(raw(true, OP_BINARY, None, b"first"));
        server_io.write_all(&answer).await.unwrap();
        server_io
    });
    let read_ahead = upgrade::connect(&mut client_io, &config).await.unwrap();
    let _server_io = server.await.unwrap();
    let (r, w) = split(client_io);
    let mut ws = WsStream::new(r, w, Role::Client, &read_ahead).unwrap();
    let mut first = [0u8; 5];
    ws.read_exact(&mut first).await.unwrap();
    assert_eq!(&first, b"first");
}

#[tokio::test]
async fn client_rejects_a_bad_accept_key_and_non_101() {
    for answer in [
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: AAAAAAAAAAAAAAAAAAAAAAAAAAA=\r\n\r\n",
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 101 Switching Protocols\r\n\r\n",
    ] {
        let (mut client_io, mut server_io) = duplex(1 << 16);
        let server = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let _ = server_io.read(&mut buf).await;
            server_io.write_all(answer.as_bytes()).await.unwrap();
            server_io
        });
        let config = ClientConfig::new(None, "127.0.0.1:80", "http", 80);
        assert!(
            upgrade::connect(&mut client_io, &config).await.is_err(),
            "{answer}"
        );
        drop(server.await);
    }
}

/// A `ws` listener on localhost with path `/tunnel`, answering upgrades in the
/// background; each accepted connection echoes what it receives.
async fn ws_listener() -> (std::net::SocketAddr, Settings) {
    let ws: WsConfig = toml::from_str("path = \"/tunnel\"").unwrap();
    let settings = Settings {
        kind: TransportKind::Ws,
        ws: Some(ws),
    };
    let tuning = Tuning::for_profile(Default::default());
    let listener = Listener::bind(&settings, "127.0.0.1:0", &tuning)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (incoming, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                if let Ok(mut stream) = incoming.establish().await {
                    let mut buf = [0u8; 1024];
                    while let Ok(n @ 1..) = stream.read(&mut buf).await {
                        stream.write_all(&buf[..n]).await.unwrap();
                        stream.flush().await.unwrap();
                    }
                }
            });
        }
    });
    (addr, settings)
}

/// Sends `request` to the listener and returns the whole answer (the listener closes).
async fn probe(addr: std::net::SocketAddr, request: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(request).await.unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer))
        .await
        .expect("listener should answer and close")
        .unwrap();
    String::from_utf8(answer).unwrap()
}

#[tokio::test]
async fn probes_get_an_nginx_404() {
    let (addr, _) = ws_listener().await;
    let upgrade = |path: &str| {
        format!(
            "GET {path} HTTP/1.1\r\nHost: example.com\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        )
    };
    for request in [
        "GET / HTTP/1.1\r\nHost: example.com\r\nUser-Agent: curl/8.0\r\n\r\n".to_owned(),
        "GET /tunnel HTTP/1.1\r\nHost: example.com\r\n\r\n".to_owned(),
        upgrade("/"),
        upgrade("/tunnel/x"),
        upgrade("/admin"),
    ] {
        let answer = probe(addr, request.as_bytes()).await;
        assert!(
            answer.starts_with("HTTP/1.1 404 Not Found\r\nServer: nginx\r\n"),
            "{request:?} got {answer:?}"
        );
        assert!(answer.ends_with("<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n"));
        assert!(!answer.to_ascii_lowercase().contains("kariz"));
    }
    // Not HTTP at all, or an oversized head: 400, like nginx.
    for request in [
        b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03\r\n\r\n".to_vec(),
        vec![b'a'; 9000],
    ] {
        let answer = probe(addr, &request).await;
        assert!(
            answer.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{answer:?}"
        );
    }
    // The right path gets the upgrade.
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(upgrade("/tunnel").as_bytes()).await.unwrap();
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).await.unwrap();
    let answer = std::str::from_utf8(&buf[..n]).unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{answer}"
    );
    assert!(answer.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
}

#[tokio::test]
async fn dialer_and_listener_talk() {
    let (addr, settings) = ws_listener().await;
    let tuning = Tuning::for_profile(Default::default());
    let dialer = Dialer::new(&settings, &addr.to_string(), &tuning).unwrap();
    let mut stream = dialer.dial().await.unwrap();
    let payload = pattern(300_000);
    let (mut r, mut w) = tokio::io::split(&mut stream);
    let write = async {
        w.write_all(&payload).await.unwrap();
        w.flush().await.unwrap();
    };
    let mut back = vec![0u8; payload.len()];
    let (_, read) = tokio::join!(write, r.read_exact(&mut back));
    read.unwrap();
    assert!(back == payload);
    assert!(stream.is_alive());

    // A dialer with the wrong path is refused with the status in the error.
    let wrong: WsConfig = toml::from_str("path = \"/other\"").unwrap();
    let settings = Settings {
        ws: Some(wrong),
        ..settings
    };
    let err = Dialer::new(&settings, &addr.to_string(), &tuning)
        .unwrap()
        .dial()
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert!(err.to_string().contains("404"), "{err}");
}
