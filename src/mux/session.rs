//! A mux session: many streams over one tunnel connection.
//!
//! One driver task per session runs three loops over the connection:
//!
//! * **reader**: decodes frames and hands data to the streams;
//! * **writer**: sends queued control frames first (`WINDOW`, `RST`, `PING`, ...), then
//!   takes turns between streams that have data, coalescing several frames per write;
//! * **keepalive**: sends `PING`s and closes the session when the peer goes silent.
//!
//! Streams never touch the connection. They move bytes in and out of their buffers in
//! the shared state and wake the writer; the per-stream credit window bounds every
//! buffer, so a stream whose reader is slow only ever stalls itself.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use bytes::{Buf, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::{interval, Instant, MissedTickBehavior};

use super::frame::{self, FrameType, Header, HEADER_LEN};
use super::{ResetReason, SessionConfig, Side};

/// Credit every stream starts with in both directions, before any `WINDOW` frame.
pub const INITIAL_WINDOW: u64 = 64 * 1024;
/// Largest `DATA` payload the writer produces.
pub const MAX_DATA_FRAME: usize = 16 * 1024;
/// Bytes a stream may have accepted from its writer but not yet framed.
const SEND_BUFFER: usize = 64 * 1024;
/// Bytes the writer gathers into one write when coalescing.
const BATCH: usize = 64 * 1024;
/// Most buffers passed to one vectored write.
const MAX_IOV: usize = 64;
/// Read buffer growth step.
const READ_CHUNK: usize = 64 * 1024;
/// Payloads smaller than this are copied out of the read buffer, so a small frame
/// waiting in a stream's queue does not keep a whole read buffer alive; larger ones are
/// shared without copying.
const COPY_BELOW: usize = 4 * 1024;
/// Stop answering pings when this much control data is already queued.
const MAX_CONTROL_BACKLOG: usize = 64 * 1024;
/// Upper bound on a stream's send credit; more means the peer is broken.
const MAX_SEND_CREDIT: u64 = 1 << 32;

struct Stream {
    // Receive side.
    recv: VecDeque<Bytes>,
    recv_buffered: usize,
    /// Credit granted to the peer and not used yet.
    recv_credit: u64,
    recv_fin: bool,
    read_waker: Option<Waker>,

    // Send side.
    /// `SYN` payload, until the writer sends it.
    syn: Option<Bytes>,
    /// `WINDOW` increment to send right after our `SYN` (the peer cannot take credit for
    /// a stream it does not know yet).
    syn_window: Option<u32>,
    send_buf: BytesMut,
    send_credit: u64,
    fin_queued: bool,
    fin_sent: bool,
    write_waker: Option<Waker>,
    /// In the writer's ready queue.
    queued: bool,

    /// Set once the stream is reset by either side.
    reset: Option<ResetReason>,
    /// The `MuxStream` handle is gone; remove the stream once nothing is pending.
    detached: bool,
}

impl Stream {
    fn new() -> Self {
        Self {
            recv: VecDeque::new(),
            recv_buffered: 0,
            recv_credit: INITIAL_WINDOW,
            recv_fin: false,
            read_waker: None,
            syn: None,
            syn_window: None,
            send_buf: BytesMut::new(),
            send_credit: INITIAL_WINDOW,
            fin_queued: false,
            fin_sent: false,
            write_waker: None,
            queued: false,
            reset: None,
            detached: false,
        }
    }

    /// Credit to hand back to the peer so it can have `window` bytes outstanding again.
    /// Only sent once at least half a window is available, to keep `WINDOW` frames rare.
    fn window_update(&mut self, window: u64) -> Option<u32> {
        if self.recv_fin || self.reset.is_some() {
            return None;
        }
        let outstanding = self.recv_credit + self.recv_buffered as u64;
        let inc = window.checked_sub(outstanding)?;
        if inc == 0 || inc < window / 2 {
            return None;
        }
        let inc = inc.min(u32::MAX as u64) as u32;
        self.recv_credit += inc as u64;
        Some(inc)
    }

    fn wake(&mut self) {
        if let Some(w) = self.read_waker.take() {
            w.wake();
        }
        if let Some(w) = self.write_waker.take() {
            w.wake();
        }
    }
}

struct State {
    streams: HashMap<u32, Stream>,
    /// Streams with something for the writer, in turn order.
    ready: VecDeque<u32>,
    /// Encoded control frames, sent before any stream data.
    control: Vec<u8>,
    next_id: u32,
    last_peer_id: u32,
    goaway: bool,
    /// Why the session closed, once it has.
    closed: Option<String>,
    incoming: Option<mpsc::UnboundedSender<(MuxStream, Bytes)>>,
    last_recv: Instant,
    ping_seq: u64,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the writer; `notify_one` keeps a permit, so no wake-up is lost.
    writer: Notify,
    closed: watch::Sender<bool>,
    config: SessionConfig,
    side: Side,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self, reason: String) {
        let incoming = {
            let mut st = self.lock();
            if st.closed.is_some() {
                return;
            }
            st.closed = Some(reason);
            for s in st.streams.values_mut() {
                s.wake();
            }
            st.incoming.take()
        };
        drop(incoming);
        self.closed.send_replace(true);
        self.writer.notify_one();
    }

    fn peer_parity(&self) -> u32 {
        match self.side {
            Side::Client => 0,
            Side::Server => 1,
        }
    }

    /// Applies one frame from the peer. An error is a protocol violation and ends the
    /// session.
    fn handle_frame(self: &Arc<Self>, h: Header, payload: Bytes) -> io::Result<()> {
        let window = self.config.stream_window as u64;
        let mut rejected = None;
        let mut notify = false;
        {
            let mut st = self.lock();
            st.last_recv = Instant::now();
            let State {
                streams,
                control,
                incoming,
                last_peer_id,
                goaway,
                ..
            } = &mut *st;
            match h.kind {
                FrameType::Syn => {
                    if h.stream % 2 != self.peer_parity() || h.stream <= *last_peer_id {
                        return Err(protocol_error("peer opened a stream with an invalid id"));
                    }
                    *last_peer_id = h.stream;
                    let full = streams.len() >= self.config.max_streams;
                    if *goaway || full || incoming.is_none() {
                        frame::put(
                            control,
                            FrameType::Rst,
                            h.stream,
                            &[ResetReason::Refused.id()],
                        );
                        notify = true;
                    } else {
                        let mut stream = Stream::new();
                        if let Some(inc) = stream.window_update(window) {
                            frame::put(control, FrameType::Window, h.stream, &inc.to_be_bytes());
                            notify = true;
                        }
                        streams.insert(h.stream, stream);
                        let handle = MuxStream {
                            shared: self.clone(),
                            id: h.stream,
                        };
                        let sent = incoming.as_ref().map(|tx| tx.send((handle, payload)));
                        if let Some(Err(e)) = sent {
                            // Dropping the handle locks the state; do it after unlocking.
                            rejected = Some(e.0 .0);
                        }
                    }
                }
                FrameType::Data => {
                    if let Some(s) = streams.get_mut(&h.stream) {
                        if s.reset.is_none() {
                            if s.recv_fin {
                                return Err(protocol_error("peer sent data after FIN"));
                            }
                            let len = payload.len() as u64;
                            if len > s.recv_credit {
                                return Err(protocol_error("peer exceeded the stream window"));
                            }
                            s.recv_credit -= len;
                            if !s.detached && !payload.is_empty() {
                                s.recv_buffered += payload.len();
                                s.recv.push_back(payload);
                                if let Some(w) = s.read_waker.take() {
                                    w.wake();
                                }
                            }
                        }
                    }
                }
                FrameType::Fin => {
                    if let Some(s) = streams.get_mut(&h.stream) {
                        s.recv_fin = true;
                        if let Some(w) = s.read_waker.take() {
                            w.wake();
                        }
                        if s.detached && s.fin_sent {
                            streams.remove(&h.stream);
                        }
                    }
                }
                FrameType::Rst => {
                    if let Some(s) = streams.get_mut(&h.stream) {
                        s.reset.get_or_insert(ResetReason::from_id(payload[0]));
                        s.send_buf.clear();
                        s.wake();
                        if s.detached {
                            streams.remove(&h.stream);
                        }
                    }
                }
                FrameType::Window => {
                    if let Some(s) = streams.get_mut(&h.stream) {
                        let inc = u32::from_be_bytes(payload[..4].try_into().unwrap()) as u64;
                        s.send_credit += inc;
                        if s.send_credit > MAX_SEND_CREDIT {
                            return Err(protocol_error("peer granted too much credit"));
                        }
                        if let Some(w) = s.write_waker.take() {
                            w.wake();
                        }
                    }
                }
                FrameType::Ping => {
                    if control.len() < MAX_CONTROL_BACKLOG {
                        frame::put(control, FrameType::Pong, 0, &payload);
                        notify = true;
                    }
                }
                FrameType::Pong | FrameType::Dgram => {}
                FrameType::GoAway => {
                    *goaway = true;
                    // No more streams will arrive.
                    drop(incoming.take());
                }
            }
        }
        drop(rejected);
        if notify {
            self.writer.notify_one();
        }
        Ok(())
    }

    /// Moves queued frames into `out`. Fails once the session is closed.
    fn next_batch(&self, out: &mut Batch) -> io::Result<()> {
        let mut st = self.lock();
        if let Some(reason) = &st.closed {
            return Err(closed_error(reason));
        }
        out.put_inline(&st.control);
        st.control.clear();
        let State { streams, ready, .. } = &mut *st;
        loop {
            let room = if self.config.coalesce {
                out.len < BATCH
            } else {
                out.len == 0
            };
            if !room {
                break;
            }
            let Some(id) = ready.pop_front() else {
                break;
            };
            let Some(s) = streams.get_mut(&id) else {
                continue;
            };
            s.queued = false;
            if s.reset.is_some() {
                continue;
            }
            if let Some(syn) = s.syn.take() {
                out.put_frame(FrameType::Syn, id, &syn);
                if let Some(inc) = s.syn_window.take() {
                    out.put_frame(FrameType::Window, id, &inc.to_be_bytes());
                }
            }
            if !s.send_buf.is_empty() {
                let n = s.send_buf.len().min(MAX_DATA_FRAME);
                out.put_data(id, s.send_buf.split_to(n).freeze());
                if let Some(w) = s.write_waker.take() {
                    w.wake();
                }
            }
            if !s.send_buf.is_empty() {
                s.queued = true;
                ready.push_back(id);
            } else if s.fin_queued && !s.fin_sent {
                out.put_frame(FrameType::Fin, id, &[]);
                s.fin_sent = true;
                if s.detached && s.recv_fin {
                    streams.remove(&id);
                }
            }
        }
        Ok(())
    }
}

/// Frames gathered for one write. Headers and small payloads are stored inline;
/// stream data stays in its `Bytes` and goes out with a vectored write, uncopied.
#[derive(Default)]
struct Batch {
    inline: Vec<u8>,
    segments: Vec<Segment>,
    len: usize,
}

enum Segment {
    Inline(Range<usize>),
    Data(Bytes),
}

impl Batch {
    fn clear(&mut self) {
        self.inline.clear();
        self.segments.clear();
        self.len = 0;
    }

    fn put_inline(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let start = self.inline.len();
        self.inline.extend_from_slice(bytes);
        let end = self.inline.len();
        match self.segments.last_mut() {
            Some(Segment::Inline(r)) if r.end == start => r.end = end,
            _ => self.segments.push(Segment::Inline(start..end)),
        }
        self.len += bytes.len();
    }

    fn put_frame(&mut self, kind: FrameType, stream: u32, payload: &[u8]) {
        self.put_inline(&frame::header(kind, stream, payload.len()));
        self.put_inline(payload);
    }

    fn put_data(&mut self, stream: u32, data: Bytes) {
        self.put_inline(&frame::header(FrameType::Data, stream, data.len()));
        self.len += data.len();
        self.segments.push(Segment::Data(data));
    }

    fn segment(&self, i: usize) -> &[u8] {
        match &self.segments[i] {
            Segment::Inline(r) => &self.inline[r.clone()],
            Segment::Data(b) => b,
        }
    }

    async fn write_to<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> io::Result<()> {
        let (mut i, mut offset) = (0, 0);
        while i < self.segments.len() {
            let slices: Vec<io::IoSlice<'_>> = (i..self.segments.len().min(i + MAX_IOV))
                .map(|k| {
                    let s = self.segment(k);
                    io::IoSlice::new(if k == i { &s[offset..] } else { s })
                })
                .collect();
            let mut n = writer.write_vectored(&slices).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            while n > 0 {
                let rest = self.segment(i).len() - offset;
                if n >= rest {
                    n -= rest;
                    i += 1;
                    offset = 0;
                } else {
                    offset += n;
                    n = 0;
                }
            }
        }
        Ok(())
    }
}

/// Handle to a mux session. Dropping it closes the session and all its streams.
pub struct MuxSession {
    shared: Arc<Shared>,
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<(MuxStream, Bytes)>>,
}

impl MuxSession {
    /// Starts a session over `io`. Both ends must use opposite `side`s.
    pub fn new<T>(io: T, side: Side, config: SessionConfig) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                streams: HashMap::new(),
                ready: VecDeque::new(),
                control: Vec::new(),
                next_id: match side {
                    Side::Client => 1,
                    Side::Server => 2,
                },
                last_peer_id: 0,
                goaway: false,
                closed: None,
                incoming: Some(tx),
                last_recv: Instant::now(),
                ping_seq: 0,
            }),
            writer: Notify::new(),
            closed: watch::channel(false).0,
            config,
            side,
        });

        let (reader, writer) = tokio::io::split(io);
        let s = shared.clone();
        tokio::spawn(async move {
            let mut closed = s.closed.subscribe();
            let result = tokio::select! {
                r = read_loop(&s, reader) => r,
                r = write_loop(&s, writer) => r,
                r = keepalive_loop(&s) => r,
                _ = closed.wait_for(|c| *c) => Ok(()),
            };
            s.close(match result {
                Ok(()) => "closed".into(),
                Err(e) => e.to_string(),
            });
        });

        Self {
            shared,
            incoming: tokio::sync::Mutex::new(rx),
        }
    }

    /// Opens a stream. `syn` (the open request) and any data written right away are sent
    /// without waiting for the peer: the peer answers a failed open with a reset.
    pub fn open(&self, syn: Bytes) -> io::Result<MuxStream> {
        if syn.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "open request too long",
            ));
        }
        let window = self.shared.config.stream_window as u64;
        let id = {
            let mut st = self.shared.lock();
            if let Some(reason) = &st.closed {
                return Err(closed_error(reason));
            }
            if st.goaway {
                return Err(io::Error::other("mux session is going away"));
            }
            if st.streams.len() >= self.shared.config.max_streams {
                return Err(io::Error::other("mux session has too many streams"));
            }
            let id = st.next_id;
            st.next_id = match id.checked_add(2) {
                Some(next) => next,
                None => {
                    st.goaway = true;
                    return Err(io::Error::other("mux stream ids exhausted"));
                }
            };
            let mut stream = Stream::new();
            stream.syn = Some(syn);
            stream.syn_window = stream.window_update(window);
            stream.queued = true;
            st.streams.insert(id, stream);
            st.ready.push_back(id);
            id
        };
        self.shared.writer.notify_one();
        Ok(MuxStream {
            shared: self.shared.clone(),
            id,
        })
    }

    /// Waits for a stream the peer opened, with its `SYN` payload. `None` once the
    /// session is closed or going away.
    pub async fn accept(&self) -> Option<(MuxStream, Bytes)> {
        self.incoming.lock().await.recv().await
    }

    /// Stops new streams in both directions; existing streams continue.
    pub fn goaway(&self) {
        let incoming = {
            let mut st = self.shared.lock();
            if st.goaway || st.closed.is_some() {
                return;
            }
            st.goaway = true;
            frame::put(&mut st.control, FrameType::GoAway, 0, &[]);
            st.incoming.take()
        };
        drop(incoming);
        self.shared.writer.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        self.shared.lock().closed.is_some()
    }

    /// Going away (either side sent `GOAWAY`) or closed: no new streams.
    pub fn is_draining(&self) -> bool {
        let st = self.shared.lock();
        st.goaway || st.closed.is_some()
    }

    pub fn stream_count(&self) -> usize {
        self.shared.lock().streams.len()
    }

    /// Resolves once the session is closed, for whatever reason.
    pub async fn closed(&self) {
        let mut rx = self.shared.closed.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }

    pub fn close(&self) {
        self.shared.close("closed locally".into());
    }
}

impl Drop for MuxSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads straight into one buffer and cuts frames out of it.
async fn read_loop<R: AsyncRead + Unpin>(shared: &Arc<Shared>, mut reader: R) -> io::Result<()> {
    let mut buf = BytesMut::with_capacity(READ_CHUNK);
    loop {
        while buf.len() >= HEADER_LEN {
            let h = Header::decode(buf[..HEADER_LEN].try_into().unwrap())?;
            let len = h.len as usize;
            if buf.len() < HEADER_LEN + len {
                break;
            }
            buf.advance(HEADER_LEN);
            let payload = if len < COPY_BELOW {
                let p = Bytes::copy_from_slice(&buf[..len]);
                buf.advance(len);
                p
            } else {
                buf.split_to(len).freeze()
            };
            shared.handle_frame(h, payload)?;
        }
        if buf.capacity() - buf.len() < READ_CHUNK / 2 {
            buf.reserve(READ_CHUNK);
        }
        if reader.read_buf(&mut buf).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "peer closed the mux session",
            ));
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(shared: &Shared, mut writer: W) -> io::Result<()> {
    let mut batch = Batch::default();
    loop {
        batch.clear();
        shared.next_batch(&mut batch)?;
        if batch.len == 0 {
            shared.writer.notified().await;
            continue;
        }
        batch.write_to(&mut writer).await?;
        writer.flush().await?;
    }
}

async fn keepalive_loop(shared: &Shared) -> io::Result<()> {
    let period = shared.config.keepalive;
    let mut tick = interval(period);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        {
            let mut st = shared.lock();
            if st.last_recv.elapsed() > period * 2 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "mux peer stopped answering pings",
                ));
            }
            st.ping_seq += 1;
            let seq = st.ping_seq.to_be_bytes();
            frame::put(&mut st.control, FrameType::Ping, 0, &seq);
        }
        shared.writer.notify_one();
    }
}

/// One stream of a [`MuxSession`]. Dropping it before both directions have finished
/// resets the stream.
pub struct MuxStream {
    shared: Arc<Shared>,
    id: u32,
}

impl MuxStream {
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Aborts the stream and tells the peer why.
    pub fn reset(self, reason: ResetReason) {
        {
            let mut st = self.shared.lock();
            let State {
                streams, control, ..
            } = &mut *st;
            if let Some(s) = streams.get_mut(&self.id) {
                if s.reset.is_none() {
                    s.reset = Some(reason);
                    frame::put(control, FrameType::Rst, self.id, &[reason.id()]);
                }
            }
        }
        self.shared.writer.notify_one();
        // Drop removes the stream.
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        let mut notify = false;
        {
            let mut st = self.shared.lock();
            let State {
                streams, control, ..
            } = &mut *st;
            let Some(s) = streams.get_mut(&self.id) else {
                return;
            };
            s.detached = true;
            s.recv.clear();
            s.recv_buffered = 0;
            if s.reset.is_some() || (s.fin_sent && s.recv_fin) {
                streams.remove(&self.id);
            } else if s.fin_queued && s.recv_fin {
                // The writer removes it after sending the queued data and FIN.
            } else {
                frame::put(
                    control,
                    FrameType::Rst,
                    self.id,
                    &[ResetReason::Cancel.id()],
                );
                streams.remove(&self.id);
                notify = true;
            }
        }
        if notify {
            self.shared.writer.notify_one();
        }
    }
}

impl AsyncRead for MuxStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let window = self.shared.config.stream_window as u64;
        let mut st = self.shared.lock();
        let State {
            streams,
            control,
            closed,
            ..
        } = &mut *st;
        let Some(s) = streams.get_mut(&self.id) else {
            return Poll::Ready(Err(stream_gone()));
        };
        if s.recv_buffered > 0 {
            let mut n = 0;
            while buf.remaining() > 0 {
                let Some(front) = s.recv.front_mut() else {
                    break;
                };
                let k = front.len().min(buf.remaining());
                buf.put_slice(&front[..k]);
                front.advance(k);
                if front.is_empty() {
                    s.recv.pop_front();
                }
                n += k;
            }
            s.recv_buffered -= n;
            if let Some(inc) = s.window_update(window) {
                frame::put(control, FrameType::Window, self.id, &inc.to_be_bytes());
                drop(st);
                self.shared.writer.notify_one();
            }
            return Poll::Ready(Ok(()));
        }
        if let Some(reason) = s.reset {
            return Poll::Ready(Err(reason.to_error()));
        }
        if s.recv_fin {
            return Poll::Ready(Ok(()));
        }
        if let Some(reason) = closed {
            return Poll::Ready(Err(closed_error(reason)));
        }
        s.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for MuxStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut st = self.shared.lock();
        let State {
            streams,
            ready,
            closed,
            ..
        } = &mut *st;
        if let Some(reason) = closed {
            return Poll::Ready(Err(closed_error(reason)));
        }
        let Some(s) = streams.get_mut(&self.id) else {
            return Poll::Ready(Err(stream_gone()));
        };
        if let Some(reason) = s.reset {
            return Poll::Ready(Err(reason.to_error()));
        }
        if s.fin_queued {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write after shutdown",
            )));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let room = (SEND_BUFFER - s.send_buf.len()) as u64;
        let n = s.send_credit.min(room).min(buf.len() as u64) as usize;
        if n == 0 {
            s.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        s.send_buf.extend_from_slice(&buf[..n]);
        s.send_credit -= n as u64;
        if !s.queued {
            s.queued = true;
            ready.push_back(self.id);
        }
        drop(st);
        self.shared.writer.notify_one();
        Poll::Ready(Ok(n))
    }

    /// Queued data is sent by the session on its own; nothing to wait for.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        {
            let mut st = self.shared.lock();
            let State {
                streams,
                ready,
                closed,
                ..
            } = &mut *st;
            if let Some(reason) = closed {
                return Poll::Ready(Err(closed_error(reason)));
            }
            let Some(s) = streams.get_mut(&self.id) else {
                return Poll::Ready(Err(stream_gone()));
            };
            if let Some(reason) = s.reset {
                return Poll::Ready(Err(reason.to_error()));
            }
            if s.fin_queued {
                return Poll::Ready(Ok(()));
            }
            s.fin_queued = true;
            if !s.queued {
                s.queued = true;
                ready.push_back(self.id);
            }
        }
        self.shared.writer.notify_one();
        Poll::Ready(Ok(()))
    }
}

fn closed_error(reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        format!("mux session closed: {reason}"),
    )
}

fn stream_gone() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "mux stream is gone")
}

fn protocol_error(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
