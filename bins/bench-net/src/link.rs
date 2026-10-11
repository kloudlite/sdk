//! Per-stream plumbing on the bench: the packet device a stream's userspace stack (`ipstack`) runs
//! on, the anti-inject rule, and the open-stream count the sessions daemon reads to stay awake
//! (`bench/sessions/main.ts` `streams()`).
use std::{
    io,
    net::Ipv4Addr,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
};

/// A decrypted packet enters the stack only when its source is the address this stream was given.
/// Without this one stream could speak as another (spec §3: "a stream cannot inject packets for
/// another stream's address").
pub fn admit(src: Ipv4Addr, stream: Ipv4Addr) -> bool {
    src == stream
}

/// Rename over, so the reader never sees a half-written number.
pub fn write_count(p: &Path, n: usize) -> io::Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, n.to_string())?;
    std::fs::rename(tmp, p)
}

/// `ipstack`'s device: reads hand it one decrypted packet each, writes take one packet each for the
/// stream to encrypt. A full outbound queue drops the packet, as a congested link would; TCP
/// retransmits, and a stalled laptop cannot grow the bench's memory.
pub struct Link {
    rx: mpsc::Receiver<Vec<u8>>,
    tx: mpsc::Sender<Vec<u8>>,
}

impl Link {
    /// (device, sender of inbound packets, receiver of outbound packets)
    pub fn new(depth: usize) -> (Link, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        let (in_tx, in_rx) = mpsc::channel(depth);
        let (out_tx, out_rx) = mpsc::channel(depth);
        (Link { rx: in_rx, tx: out_tx }, in_tx, out_rx)
    }
}

impl AsyncRead for Link {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.rx.poll_recv(cx) {
            // ponytail: an over-MTU packet is truncated; the laptop utun MTU matches ours (Task 7)
            Poll::Ready(Some(p)) => {
                let n = p.len().min(buf.remaining());
                buf.put_slice(&p[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Link {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, b: &[u8]) -> Poll<io::Result<usize>> {
        let _ = self.tx.try_send(b.to_vec());
        Poll::Ready(Ok(b.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn a_packet_from_the_wrong_address_is_dropped() {
        let a: Ipv4Addr = "100.96.0.2".parse().unwrap();
        let b: Ipv4Addr = "100.96.0.3".parse().unwrap();
        assert!(admit(a, a));
        assert!(!admit(b, a)); // stream a claiming b's source
    }

    #[test]
    fn the_count_file_holds_the_number() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("net.streams");
        write_count(&p, 3).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "3");
        write_count(&p, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "0");
    }

    #[tokio::test]
    async fn the_link_moves_whole_packets_both_ways() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut link, inbound, mut outbound) = Link::new(4);
        inbound.send(vec![1, 2, 3]).await.unwrap();
        let mut buf = [0u8; 1500];
        assert_eq!(link.read(&mut buf).await.unwrap(), 3); // one read = one packet
        assert_eq!(&buf[..3], &[1, 2, 3]);
        link.write_all(&[9, 9]).await.unwrap();
        assert_eq!(outbound.recv().await.unwrap(), vec![9, 9]); // one write = one packet
        drop(inbound);
        assert_eq!(link.read(&mut buf).await.unwrap(), 0); // stream gone = EOF
    }
}
