//! Nonblocking TCP and pipe wake on macOS.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::Duration;

use super::socket::to_storage;
use super::wake::{Wake, WakeHandle};
use crate::tcp::{Ready, normalize};

#[derive(Debug)]
pub(crate) struct Tcp {
    socket: TcpStream,
    wake: Wake,
    write_progress: bool,
}

impl Tcp {
    /// No platform marking on macOS; the shared guard reports it as not applied.
    pub(crate) fn qos(&self, requested: bool) -> evoparseq_socket_qos::Flow {
        evoparseq_socket_qos::Flow::new(&self.socket, requested, "BUD")
    }

    pub(crate) fn start(server: SocketAddr) -> io::Result<Self> {
        let raw = unsafe {
            libc::socket(
                libc::AF_INET6,
                libc::SOCK_STREAM,
                libc::IPPROTO_TCP,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            let flags = libc::fcntl(raw, libc::F_GETFL, 0);
            libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
            libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let socket = unsafe { TcpStream::from_raw_fd(raw) };
        let dual_stack: libc::c_int = 0;
        if unsafe {
            libc::setsockopt(
                raw,
                libc::IPPROTO_IPV6,
                libc::IPV6_V6ONLY,
                core::ptr::addr_of!(dual_stack).cast(),
                super::socket::socklen(core::mem::size_of_val(&dual_stack)),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let (addr, len) = to_storage(server);
        if unsafe { libc::connect(raw, core::ptr::addr_of!(addr).cast(), len) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
        }
        Ok(Self {
            socket,
            wake: Wake::new()?,
            write_progress: false,
        })
    }

    pub(crate) fn notify_handle(&self) -> io::Result<WakeHandle> {
        self.wake.handle()
    }

    pub(crate) fn poll(&mut self, timeout: Duration, write: bool) -> io::Result<Ready> {
        let mut fds = [
            libc::pollfd {
                fd: self.socket.as_raw_fd(),
                events: libc::POLLIN | if write { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: self.wake.raw(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let millis = timeout.as_millis() + u128::from(timeout.subsec_nanos() % 1_000_000 != 0);
        let millis = i32::try_from(millis).unwrap_or(i32::MAX);
        let count = unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let [socket, wake] = fds;
        if wake.revents != 0 {
            self.wake.take()?;
        }
        Ok(Ready {
            readable: socket.revents & (libc::POLLIN | libc::POLLHUP) != 0,
            writable: socket.revents & libc::POLLOUT != 0,
            error: socket.revents & (libc::POLLERR | libc::POLLNVAL) != 0,
        })
    }

    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.socket.take_error()
    }
    pub(crate) fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.socket.peer_addr().map(normalize)
    }
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr().map(normalize)
    }
    pub(crate) fn set_nodelay(&self, enabled: bool) -> io::Result<()> {
        self.socket.set_nodelay(enabled)
    }
    #[cfg(test)]
    pub(crate) fn nodelay(&self) -> io::Result<bool> {
        self.socket.nodelay()
    }
    pub(crate) fn pending_write(&self) -> bool {
        false
    }
    pub(crate) fn take_write_progress(&mut self) -> bool {
        core::mem::take(&mut self.write_progress)
    }
}

impl Read for Tcp {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket.read(bytes)
    }
}
impl Write for Tcp {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.socket.write(bytes)?;
        self.write_progress |= count != 0;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
