//! Stands in for `kcp/` in builds without the `kcp` feature. Streams and listeners have
//! no values, so every path that would use one is unreachable, and the compiler knows
//! it. Config validation rejects `transport = "kcp"` in such a build; binding or dialing
//! anyway fails.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::config::{Tuning, TunnelConfig};

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "this build has no KCP support (the `kcp` feature is off)",
    )
}

#[derive(Clone, Debug, Default)]
pub struct KcpParams;

impl KcpParams {
    pub fn new(_: &TunnelConfig, _: crate::config::KcpConfig) -> Self {
        Self
    }
}

pub enum KcpListener {}

impl KcpListener {
    pub async fn bind(_: &str, _: &KcpParams, _: &Tuning) -> io::Result<Self> {
        Err(unsupported())
    }

    pub async fn accept(&self) -> io::Result<(KcpStream, SocketAddr)> {
        match *self {}
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match *self {}
    }
}

pub struct KcpDialer;

impl KcpDialer {
    pub fn new(_: &str, _: &KcpParams, _: &Tuning) -> Self {
        Self
    }

    pub async fn dial(&self) -> io::Result<KcpStream> {
        Err(unsupported())
    }
}

pub enum KcpStream {}

pub enum KcpReader {}

pub enum KcpWriter {}

pub enum KcpDatagrams {}

impl KcpDatagrams {
    pub fn max_len(&self) -> usize {
        match *self {}
    }

    pub fn send(&self, _: &[u8], _: bool) -> bool {
        match *self {}
    }

    pub async fn recv(&self) -> Option<bytes::Bytes> {
        match *self {}
    }
}

impl KcpStream {
    pub fn into_split(self) -> (KcpReader, KcpWriter) {
        match self {}
    }

    pub fn datagrams(&self) -> Option<KcpDatagrams> {
        match *self {}
    }

    pub fn is_alive(&self) -> bool {
        match *self {}
    }
}

macro_rules! no_io {
    ($($ty:ty),*) => {$(
        impl AsyncRead for $ty {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                _: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                match *self {}
            }
        }

        impl AsyncWrite for $ty {
            fn poll_write(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                _: &[u8],
            ) -> Poll<io::Result<usize>> {
                match *self {}
            }

            fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
                match *self {}
            }

            fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
                match *self {}
            }
        }
    )*};
}

no_io!(KcpStream, KcpReader, KcpWriter);
