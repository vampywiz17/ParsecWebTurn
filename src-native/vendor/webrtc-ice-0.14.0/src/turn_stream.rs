//! RFC 8656 section 7: STUN and padded ChannelData messages over TCP/TLS.
//! The relay allocation remains UDP; only the client-to-TURN leg is a stream.
use async_trait::async_trait;
use std::{any::Any, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    sync::{watch, Mutex},
};
use util::conn::Conn;

trait Io: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Io for T {}
type Stream = Box<dyn Io>;

/// The upstream TURN client does not own/close its supplied transport.
/// Bind stream lifetime to the allocation so ICE shutdown closes both.
pub(crate) struct RelayStream {
    pub relay: Arc<dyn Conn + Send + Sync>,
    pub transport: Arc<dyn Conn + Send + Sync>,
}
#[async_trait]
impl Conn for RelayStream {
    async fn connect(&self, addr: SocketAddr) -> util::error::Result<()> {
        self.relay.connect(addr).await
    }
    async fn recv(&self, buf: &mut [u8]) -> util::error::Result<usize> {
        self.relay.recv(buf).await
    }
    async fn recv_from(&self, buf: &mut [u8]) -> util::error::Result<(usize, SocketAddr)> {
        self.relay.recv_from(buf).await
    }
    async fn send(&self, buf: &[u8]) -> util::error::Result<usize> {
        self.relay.send(buf).await
    }
    async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> util::error::Result<usize> {
        self.relay.send_to(buf, addr).await
    }
    fn local_addr(&self) -> util::error::Result<SocketAddr> {
        self.relay.local_addr()
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.relay.remote_addr()
    }
    async fn close(&self) -> util::error::Result<()> {
        let relay = self.relay.close().await;
        let transport = self.transport.close().await;
        relay.and(transport)
    }
    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }
}

pub struct TurnStream {
    reader: Mutex<ReadHalf<Stream>>,
    writer: Mutex<WriteHalf<Stream>>,
    local: SocketAddr,
    remote: SocketAddr,
    closed: watch::Sender<bool>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid TURN stream frame")
}

/// Return the logical frame size and transport padding after a four-byte header.
fn frame_shape(header: &[u8]) -> io::Result<(usize, usize)> {
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    match header[0] >> 6 {
        0 if length % 4 == 0 => Ok((20 + length, 0)),
        1 => Ok((4 + length, (4 - length % 4) % 4)),
        _ => Err(invalid()),
    }
}

impl TurnStream {
    /// TLS always validates the server hostname and public certificate chain.
    /// No permissive verifier or legacy Parsec DTLS option applies to TURN TLS.
    pub async fn dial(host: &str, port: u16, tls: bool) -> io::Result<Self> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let addresses: Vec<_> = tokio::net::lookup_host((host, port))
                .await?
                .filter(SocketAddr::is_ipv4)
                .collect();
            // ICE relay gathering currently uses UDP4. Try every matching DNS
            // address rather than failing on the first unreachable endpoint.
            if addresses.is_empty() {
                return Err(io::Error::other("TURN server has no IPv4 address"));
            }
            let tcp = TcpStream::connect(addresses.as_slice()).await?;
            tcp.set_nodelay(true)?;
            let local = tcp.local_addr()?;
            let remote = tcp.peer_addr()?;
            let stream: Stream = if tls {
                let mut roots = rustls::RootCertStore::from_iter(
                    webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
                );
                // Include OS-trusted enterprise/self-hosted TURN CAs. Trust is
                // still explicit at OS level; hostname verification is retained.
                roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
                let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .map_err(io::Error::other)?
                .with_root_certificates(roots)
                .with_no_client_auth();
                let name = rustls::pki_types::ServerName::try_from(host.to_owned())
                    .map_err(|_| invalid())?;
                Box::new(
                    tokio_rustls::TlsConnector::from(Arc::new(config))
                        .connect(name, tcp)
                        .await?,
                )
            } else {
                Box::new(tcp)
            };
            Ok(Self::from_stream(stream, local, remote))
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TURN connect timed out"))?
    }

    fn from_stream(stream: Stream, local: SocketAddr, remote: SocketAddr) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            local,
            remote,
            closed: watch::channel(false).0,
        }
    }
}

#[async_trait]
impl Conn for TurnStream {
    async fn connect(&self, address: SocketAddr) -> util::error::Result<()> {
        if address != self.remote {
            return Err(invalid().into());
        }
        Ok(())
    }
    async fn recv(&self, buffer: &mut [u8]) -> util::error::Result<usize> {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "TURN stream closed").into());
        }
        tokio::select! {
            biased;
            _ = closed.changed() => Err(io::Error::new(io::ErrorKind::NotConnected, "TURN stream closed").into()),
            result = async {
                let mut reader = self.reader.lock().await;
                let mut header = [0; 4];
                reader.read_exact(&mut header).await?;
                let (size, padding) = frame_shape(&header)?;
                if size > buffer.len() { return Err(invalid()); }
                buffer[..4].copy_from_slice(&header);
                reader.read_exact(&mut buffer[4..size]).await?;
                if header[0] >> 6 == 0 && buffer[4..8] != [0x21, 0x12, 0xa4, 0x42] {
                    return Err(invalid());
                }
                let mut pad = [0; 3];
                reader.read_exact(&mut pad[..padding]).await?;
                Ok::<_, io::Error>(size)
            } => {
                // A short read or malformed frame destroys stream alignment.
                // Make failure terminal rather than interpreting payload as a
                // subsequent header or spinning on EOF in the TURN reader.
                if result.is_err() { self.closed.send_replace(true); }
                result.map_err(Into::into)
            },
        }
    }
    async fn recv_from(&self, buffer: &mut [u8]) -> util::error::Result<(usize, SocketAddr)> {
        Ok((self.recv(buffer).await?, self.remote))
    }
    async fn send(&self, buffer: &[u8]) -> util::error::Result<usize> {
        if buffer.len() < 4 {
            return Err(invalid().into());
        }
        let (size, padding) = frame_shape(buffer)?;
        // Some TURN implementations already pad ChannelData on UDP as well.
        if buffer.len() != size && buffer.len() != size + padding {
            return Err(invalid().into());
        }
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "TURN stream closed").into());
        }
        tokio::select! {
            biased;
            _ = closed.changed() => Err(io::Error::new(io::ErrorKind::NotConnected, "TURN stream closed").into()),
            result = async {
                let mut writer = self.writer.lock().await;
                writer.write_all(&buffer[..size]).await?;
                writer.write_all(&[0; 3][..padding]).await?;
                writer.flush().await?;
                Ok::<_, io::Error>(buffer.len())
            } => result.map_err(Into::into),
        }
    }
    async fn send_to(&self, buffer: &[u8], target: SocketAddr) -> util::error::Result<usize> {
        self.connect(target).await?;
        self.send(buffer).await
    }
    fn local_addr(&self) -> util::error::Result<SocketAddr> {
        Ok(self.local)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
    async fn close(&self) -> util::error::Result<()> {
        self.closed.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            self.writer.lock().await.shutdown().await
        })
        .await;
        Ok(())
    }
    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }
}
