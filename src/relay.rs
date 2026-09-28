//! Bidirectional byte relay between a user connection and a tunnel connection.

use std::io;

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::session::SessionStream;

/// Copies data in both directions until both sides are done.
/// Returns `(a_to_b, b_to_a)` byte counts.
pub async fn relay<A, B>(a: &mut A, b: &mut B, buffer_size: usize) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    tokio::io::copy_bidirectional_with_sizes(a, b, buffer_size, buffer_size).await
}

/// Relays between a TCP connection and a session stream without copying payloads in
/// user space: what is read from the socket is handed to the session as is, and what
/// arrives on the stream is written to the socket straight from the frames it came in.
/// Returns `(socket_to_stream, stream_to_socket)` byte counts.
pub async fn relay_stream(
    socket: &mut TcpStream,
    stream: &SessionStream,
    buffer_size: usize,
) -> io::Result<(u64, u64)> {
    let (mut from_socket, mut to_socket) = socket.split();
    let up = async {
        let mut buf = BytesMut::new();
        let mut total = 0u64;
        loop {
            // Chunks already handed to the mux keep their part of the allocation alive;
            // start a fresh one once little room is left.
            if buf.capacity() < buffer_size / 4 {
                buf.reserve(buffer_size);
            }
            if from_socket.read_buf(&mut buf).await? == 0 {
                stream.finish()?;
                return Ok(total);
            }
            total += buf.len() as u64;
            stream.send(buf.split().freeze()).await?;
        }
    };
    let down = async {
        let mut total = 0u64;
        while let Some(chunk) = stream.recv().await? {
            to_socket.write_all(&chunk).await?;
            total += chunk.len() as u64;
        }
        to_socket.shutdown().await?;
        Ok(total)
    };
    tokio::try_join!(up, down)
}
