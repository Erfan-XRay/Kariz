//! WebSocket transport (`ws`): the tunnel connection looks like a browser WebSocket, so
//! it passes through CDNs and HTTP reverse proxies.
//!
//! A small RFC 6455 implementation instead of `tungstenite`: the tunnel needs a byte
//! stream (`AsyncRead` + `AsyncWrite`), not messages, so payloads are streamed through
//! without buffering whole frames, and outgoing data is framed in one copy.
//!
//! * [`upgrade`]: the HTTP/1.1 upgrade on both sides, and the `404` for anything else.
//! * [`frame`]: header codec and masking.
//! * [`WsReader`] / [`WsWriter`]: binary frames in both directions, as independent
//!   halves. Pings are answered, a close frame reads as end of stream, and shutting the
//!   writer down sends a close frame.

pub mod frame;
pub mod upgrade;

#[cfg(test)]
mod tests;

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use frame::{
    apply_mask, Header, MaskKeys, MAX_CONTROL_PAYLOAD, MAX_HEADER_LEN, OP_BINARY, OP_CLOSE,
    OP_CONTINUATION, OP_PING, OP_PONG, OP_TEXT,
};

pub use upgrade::{Accepted, ClientConfig, ServerConfig};

/// Incoming data frames larger than this are a protocol error. Payloads are streamed,
/// so this is a sanity limit, not a buffer size.
pub const MAX_FRAME_IN: u64 = 1 << 20;
/// Largest data frame this side sends; bigger writes become several frames. Holds one
/// full write batch of the record layer.
pub const MAX_FRAME_OUT: usize = 128 * 1024;
/// Bytes read ahead from the connection while looking for frame headers.
const READ_BUFFER: usize = 16 * 1024;
/// Close status 1000, "normal closure".
const CLOSE_NORMAL: [u8; 2] = 1000u16.to_be_bytes();

/// Which end of the WebSocket connection this is. The client masks what it sends and
/// the server must not, so each side rejects frames with the wrong masking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

/// Control replies the reader owes the peer; the writer sends them.
#[derive(Default)]
struct Shared {
    /// Fast path for the writer: set while `pong` holds a payload.
    pong_pending: AtomicBool,
    pong: Mutex<Option<Vec<u8>>>,
}

impl Shared {
    fn queue_pong(&self, payload: &[u8]) {
        // Only the latest ping needs an answer (RFC 6455, section 5.5.3).
        *self.pong.lock().unwrap_or_else(|e| e.into_inner()) = Some(payload.to_vec());
        self.pong_pending.store(true, Ordering::Release);
    }

    fn take_pong(&self) -> Option<Vec<u8>> {
        if !self.pong_pending.swap(false, Ordering::Acquire) {
            return None;
        }
        self.pong.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// A WebSocket connection carrying a byte stream in binary frames.
pub struct WsStream<R, W> {
    reader: WsReader<R>,
    writer: WsWriter<W>,
}

impl<R, W> WsStream<R, W> {
    /// Wraps the two halves of a connection whose upgrade is done. `read_ahead` holds
    /// bytes that arrived after the HTTP head and belong to the first frames.
    pub fn new(reader: R, writer: W, role: Role, read_ahead: &[u8]) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        Ok(Self {
            reader: WsReader::new(reader, role, read_ahead, shared.clone()),
            writer: WsWriter::new(writer, role, shared)?,
        })
    }

    /// Listening side: wraps a connection whose upgrade request was accepted. The `101`
    /// goes out with the first write or flush, unless [`Self::reject_upgrade`] comes
    /// first; early data from the request is read before any frame.
    pub fn accepted(reader: R, writer: W, accepted: Accepted) -> io::Result<Self> {
        let mut this = Self::new(reader, writer, Role::Server, &accepted.read_ahead)?;
        this.reader.early = accepted.early;
        this.writer.buf = accepted.response;
        this.writer.upgrade_pending = true;
        Ok(this)
    }

    /// Replaces a `101` that has not gone out yet with a `404`, and makes this stream
    /// write nothing else. Returns whether it did; shut the stream down afterwards to
    /// send it.
    pub fn reject_upgrade(&mut self) -> bool {
        self.writer.reject_upgrade()
    }

    pub fn reader(&self) -> &WsReader<R> {
        &self.reader
    }

    pub fn into_split(self) -> (WsReader<R>, WsWriter<W>) {
        (self.reader, self.writer)
    }
}

#[derive(Debug, Clone, Copy)]
enum ReadState {
    /// Waiting for the next frame header.
    Header,
    /// Inside the payload of a data frame.
    Payload {
        remaining: u64,
        mask: Option<[u8; 4]>,
        offset: usize,
    },
    /// A close frame was received: end of stream.
    Closed,
}

/// Receiving half: yields the payloads of binary frames as one byte stream.
pub struct WsReader<R> {
    inner: R,
    /// Payload bytes that came as early data in the upgrade request; read first.
    early: Vec<u8>,
    early_pos: usize,
    /// Read-ahead bytes in `buf[start..end]`.
    buf: Box<[u8]>,
    start: usize,
    end: usize,
    state: ReadState,
    /// Frames from the peer must be masked (we are the server).
    expect_masked: bool,
    /// A fragmented message is in progress, so the next data frame is a continuation.
    in_message: bool,
    shared: Arc<Shared>,
}

impl<R> WsReader<R> {
    fn new(inner: R, role: Role, read_ahead: &[u8], shared: Arc<Shared>) -> Self {
        let mut buf = vec![0u8; READ_BUFFER.max(read_ahead.len())].into_boxed_slice();
        buf[..read_ahead.len()].copy_from_slice(read_ahead);
        Self {
            inner,
            early: Vec::new(),
            early_pos: 0,
            buf,
            start: 0,
            end: read_ahead.len(),
            state: ReadState::Header,
            expect_masked: role == Role::Server,
            in_message: false,
            shared,
        }
    }

    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Whether early data from the upgrade request is still unread.
    pub fn has_early(&self) -> bool {
        self.early_pos < self.early.len()
    }

    /// Whether bytes from the peer are waiting in the read-ahead buffer.
    pub fn has_buffered(&self) -> bool {
        self.start < self.end
    }

    fn buffered(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }

    /// Handles a complete control frame at the start of the buffer.
    fn control(&mut self, header: Header, header_len: usize) {
        let from = self.start + header_len;
        let payload = &mut self.buf[from..from + header.len as usize];
        if let Some(key) = header.mask {
            apply_mask(payload, key, 0);
        }
        match header.opcode {
            OP_PING => self.shared.queue_pong(payload),
            OP_CLOSE => self.state = ReadState::Closed,
            _ => {} // unsolicited pong
        }
        self.start = from + header.len as usize;
    }

    /// The client masks every frame, the server none.
    fn check_mask(&self, header: &Header) -> io::Result<()> {
        if header.mask.is_some() == self.expect_masked {
            return Ok(());
        }
        Err(protocol(if self.expect_masked {
            "unmasked websocket frame from the client"
        } else {
            "masked websocket frame from the server"
        }))
    }

    /// Checks a data frame header against the message sequence and starts its payload.
    fn data(&mut self, header: Header) -> io::Result<()> {
        match (header.opcode, self.in_message) {
            (OP_BINARY, false) | (OP_CONTINUATION, true) => {}
            (OP_TEXT, _) => return Err(protocol("websocket text frames are not used")),
            _ => return Err(protocol("websocket message fragments out of order")),
        }
        if header.len > MAX_FRAME_IN {
            return Err(protocol("websocket frame too large"));
        }
        self.in_message = !header.fin;
        self.state = ReadState::Payload {
            remaining: header.len,
            mask: header.mask,
            offset: 0,
        };
        Ok(())
    }
}

impl<R: AsyncRead + Unpin> WsReader<R> {
    /// Reads more bytes into the read-ahead buffer. Returns how many (0 at end of stream).
    fn poll_fill(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        if self.start == self.end {
            (self.start, self.end) = (0, 0);
        } else if self.buf.len() - self.end < MAX_HEADER_LEN + MAX_CONTROL_PAYLOAD {
            self.buf.copy_within(self.start..self.end, 0);
            (self.start, self.end) = (0, self.end - self.start);
        }
        let mut rb = ReadBuf::new(&mut self.buf[self.end..]);
        ready!(Pin::new(&mut self.inner).poll_read(cx, &mut rb))?;
        let n = rb.filled().len();
        self.end += n;
        Poll::Ready(Ok(n))
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for WsReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.early_pos < this.early.len() {
            let n = out.remaining().min(this.early.len() - this.early_pos);
            out.put_slice(&this.early[this.early_pos..this.early_pos + n]);
            this.early_pos += n;
            if this.early_pos == this.early.len() {
                this.early = Vec::new();
                this.early_pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        loop {
            match this.state {
                ReadState::Closed => return Poll::Ready(Ok(())),
                ReadState::Payload { remaining: 0, .. } => this.state = ReadState::Header,
                ReadState::Payload {
                    remaining,
                    mask,
                    offset,
                } => {
                    if out.remaining() == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    let want = (out.remaining() as u64).min(remaining) as usize;
                    let before = out.filled().len();
                    let n = if this.has_buffered() {
                        let n = want.min(this.end - this.start);
                        out.put_slice(&this.buf[this.start..this.start + n]);
                        this.start += n;
                        n
                    } else {
                        // Nothing read ahead: read straight into the caller's buffer.
                        let mut rb = ReadBuf::new(out.initialize_unfilled_to(want));
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        let n = rb.filled().len();
                        if n == 0 {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "connection closed inside a websocket frame",
                            )));
                        }
                        out.advance(n);
                        n
                    };
                    if let Some(key) = mask {
                        apply_mask(&mut out.filled_mut()[before..before + n], key, offset);
                    }
                    this.state = ReadState::Payload {
                        remaining: remaining - n as u64,
                        mask,
                        offset: offset + n,
                    };
                    return Poll::Ready(Ok(()));
                }
                ReadState::Header => {
                    let parsed = Header::decode(this.buffered())?;
                    if let Some((h, _)) = &parsed {
                        this.check_mask(h)?;
                    }
                    let complete = match parsed {
                        Some((h, n)) if h.is_control() => {
                            let whole = this.end - this.start >= n + h.len as usize;
                            if whole {
                                this.control(h, n);
                            }
                            whole
                        }
                        Some((h, n)) => {
                            this.data(h)?;
                            this.start += n;
                            true
                        }
                        None => false,
                    };
                    if complete {
                        continue;
                    }
                    if ready!(this.poll_fill(cx))? == 0 {
                        if this.has_buffered() || this.in_message {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "connection closed inside a websocket frame",
                            )));
                        }
                        // Closed without a close frame, between messages: end of stream.
                        this.state = ReadState::Closed;
                    }
                }
            }
        }
    }
}

/// Sending half: frames everything written as binary frames.
pub struct WsWriter<W> {
    inner: W,
    /// Mask keys; only the client masks.
    mask: Option<MaskKeys>,
    /// Encoded frames not yet written to `inner`, from `pos`.
    buf: Vec<u8>,
    pos: usize,
    /// `buf` holds only a `101` response that may still be swapped for a `404`.
    upgrade_pending: bool,
    close_sent: bool,
    shared: Arc<Shared>,
}

impl<W> WsWriter<W> {
    fn new(inner: W, role: Role, shared: Arc<Shared>) -> io::Result<Self> {
        Ok(Self {
            inner,
            mask: match role {
                Role::Client => Some(MaskKeys::new()?),
                Role::Server => None,
            },
            buf: Vec::new(),
            pos: 0,
            upgrade_pending: false,
            close_sent: false,
            shared,
        })
    }

    /// Appends one frame carrying `parts` (at most [`MAX_FRAME_OUT`] bytes in total).
    fn encode(&mut self, opcode: u8, parts: &[&[u8]]) {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let mask = self.mask.as_mut().map(MaskKeys::next_key);
        Header {
            fin: true,
            opcode,
            mask,
            len: len as u64,
        }
        .encode(&mut self.buf);
        let payload_start = self.buf.len();
        for part in parts {
            self.buf.extend_from_slice(part);
        }
        if let Some(key) = mask {
            apply_mask(&mut self.buf[payload_start..], key, 0);
        }
    }

    fn reject_upgrade(&mut self) -> bool {
        if !self.upgrade_pending {
            return false;
        }
        self.upgrade_pending = false;
        self.buf = upgrade::not_found();
        // No close frame after an HTTP error page.
        self.close_sent = true;
        true
    }

    /// Queues the pong the reader asked for, if any. Control frames go between whole
    /// data frames, never inside one.
    fn queue_control(&mut self) {
        if let Some(payload) = self.shared.take_pong() {
            self.encode(OP_PONG, &[&payload]);
        }
    }
}

impl<W: AsyncWrite + Unpin> WsWriter<W> {
    /// Writes out encoded frames that are still buffered.
    fn poll_buffered(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.pos < self.buf.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.buf[self.pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.pos += n;
        }
        self.buf.clear();
        self.pos = 0;
        Poll::Ready(Ok(()))
    }

    /// Frames `parts` (already cut to [`MAX_FRAME_OUT`]) and starts sending right away;
    /// whatever does not fit goes out on the next write or flush.
    fn poll_send(&mut self, cx: &mut Context<'_>, parts: &[&[u8]]) -> Poll<io::Result<usize>> {
        // A held-back 101 goes out in the same write as the first frame.
        if !std::mem::take(&mut self.upgrade_pending) {
            ready!(self.poll_buffered(cx))?;
        }
        if self.close_sent {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "websocket already closed",
            )));
        }
        let taken: usize = parts.iter().map(|p| p.len()).sum();
        if taken == 0 {
            return Poll::Ready(Ok(0));
        }
        self.queue_control();
        self.encode(OP_BINARY, parts);
        if let Poll::Ready(Err(e)) = self.poll_buffered(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(taken))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for WsWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let take = buf.len().min(MAX_FRAME_OUT);
        self.get_mut().poll_send(cx, &[&buf[..take]])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        // Several buffers become one frame, up to the frame size.
        let mut parts: Vec<&[u8]> = Vec::with_capacity(bufs.len());
        let mut room = MAX_FRAME_OUT;
        for b in bufs {
            if room == 0 {
                break;
            }
            let take = b.len().min(room);
            parts.push(&b[..take]);
            room -= take;
        }
        self.get_mut().poll_send(cx, &parts)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.upgrade_pending = false;
        if !this.close_sent {
            this.queue_control();
        }
        ready!(this.poll_buffered(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    /// Sends a close frame, then shuts the connection's sending side down.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.upgrade_pending = false;
        ready!(this.poll_buffered(cx))?;
        if !this.close_sent {
            this.encode(OP_CLOSE, &[&CLOSE_NORMAL]);
            this.close_sent = true;
            ready!(this.poll_buffered(cx))?;
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

impl<R: AsyncRead + Unpin, W: Unpin> AsyncRead for WsStream<R, W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(cx, buf)
    }
}

impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for WsStream<R, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(cx)
    }
}

fn protocol(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
