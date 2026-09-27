//! Bidirectional byte relay between a user connection and a tunnel connection.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

/// Copies data in both directions until both sides are done.
/// Returns `(a_to_b, b_to_a)` byte counts.
pub async fn relay<A, B>(a: &mut A, b: &mut B, buffer_size: usize) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    tokio::io::copy_bidirectional_with_sizes(a, b, buffer_size, buffer_size).await
}
