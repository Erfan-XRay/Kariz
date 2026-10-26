//! A socket below quinn (and below the seal of `quic_obfs`) that counts every datagram
//! it sends and receives on the wire ([`crate::wire`]), as the network sees it.

use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};

use crate::wire::{meter, Leg, Path};

#[derive(Debug)]
pub struct CountedSocket {
    inner: Arc<dyn AsyncUdpSocket>,
}

impl CountedSocket {
    pub fn new(inner: Arc<dyn AsyncUdpSocket>) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

/// Datagrams in a batch of `len` bytes cut every `segment` bytes (GSO and GRO).
fn datagrams(len: usize, segment: usize) -> u64 {
    len.div_ceil(segment.max(1)) as u64
}

impl AsyncUdpSocket for CountedSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.inner.try_send(transmit)?;
        let len = transmit.contents.len();
        meter().sent_many(
            Path::new(Leg::Link, transmit.destination),
            len as u64,
            datagrams(len, transmit.segment_size.unwrap_or(len)),
        );
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let received = ready!(self.inner.poll_recv(cx, bufs, meta))?;
        for m in &meta[..received] {
            meter().received_many(
                Path::new(Leg::Link, m.addr),
                m.len as u64,
                datagrams(m.len, m.stride),
            );
        }
        Poll::Ready(Ok(received))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_is_counted_as_its_datagrams() {
        assert_eq!(datagrams(1200, 1200), 1);
        assert_eq!(datagrams(3000, 1200), 3);
        assert_eq!(datagrams(0, 0), 0);
    }
}
