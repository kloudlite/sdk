//! Length framing for the tunnel stream: both legs (laptop <-> gateway <-> bench) are byte streams,
//! so a WireGuard datagram needs its boundary back. 2-byte big-endian length, then payload.
//! Length 0 is the polite close; a short read is a broken stream.
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// EOF at the length prefix means the far side closed (`None`); EOF inside a payload is an error.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut [u8; 65535],
) -> io::Result<Option<usize>> {
    let mut len = [0u8; 2];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u16::from_be_bytes(len) as usize;
    if n == 0 {
        return Ok(None);
    }
    r.read_exact(&mut buf[..n]).await?;
    Ok(Some(n))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > 65535 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame payload must be 1..=65535 bytes",
        ));
    }
    w.write_all(&(payload.len() as u16).to_be_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn split_and_joined_reads_give_whole_frames() {
        let (mut a, mut b) = tokio::io::duplex(8); // 8-byte pipe forces split reads
        let w = tokio::spawn(async move {
            write_frame(&mut a, b"hello").await.unwrap();
            write_frame(&mut a, &[7u8; 300]).await.unwrap();
        });
        let mut buf = [0u8; 65535];
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), Some(5));
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), Some(300));
        assert!(buf[..300].iter().all(|&x| x == 7));
        w.await.unwrap();
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), None); // EOF at a boundary
    }

    #[tokio::test]
    async fn zero_length_closes_and_short_read_is_an_error() {
        let mut buf = [0u8; 65535];
        assert_eq!(read_frame(&mut &[0u8, 0][..], &mut buf).await.unwrap(), None);
        let e = read_frame(&mut &[0u8, 9, 1, 2][..], &mut buf).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(write_frame(&mut Vec::new(), b"").await.is_err());
    }
}
