//! Segmentation offload and source-pinned sends on macOS.

use core::net::{IpAddr, SocketAddr};
use std::io;

use super::socket::Socket;

pub(crate) fn offload_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EOPNOTSUPP | libc::EINVAL | libc::EMSGSIZE | libc::ENOTSUP)
    )
}

pub(crate) fn offload_send(
    _socket: &Socket,
    _staged: &[u8],
    _segment: usize,
    _to: SocketAddr,
    _from: Option<IpAddr>,
) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

pub(crate) fn pinned_send(
    socket: &Socket,
    datagram: &[u8],
    to: SocketAddr,
    source: IpAddr,
) -> io::Result<()> {
    socket.send_msg(datagram, to, Some(source)).map(|_| ())
}
