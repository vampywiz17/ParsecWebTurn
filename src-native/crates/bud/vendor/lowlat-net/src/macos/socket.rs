//! The media socket through the kernel's own interface on macOS.

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use core::time::Duration;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::socket::{DEFAULT_TTL, DSCP_EF, WANT_RCVBUF, WANT_SNDBUF};

pub(super) fn socklen(bytes: usize) -> libc::socklen_t {
    libc::socklen_t::try_from(bytes).unwrap_or(libc::socklen_t::MAX)
}

fn family_v6() -> libc::sa_family_t {
    libc::sa_family_t::try_from(libc::AF_INET6).unwrap_or(0)
}

#[derive(Debug)]
pub struct Socket {
    fd: OwnedFd,
    granted_rcvbuf: i32,
    granted_sndbuf: i32,
}

impl Socket {
    pub(crate) fn bound(port: u16) -> io::Result<Self> {
        let raw = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let socket = Self {
            fd,
            granted_rcvbuf: 0,
            granted_sndbuf: 0,
        };
        socket.configure()?;
        socket.bind(port)?;

        let granted_rcvbuf = socket.get_int(libc::SOL_SOCKET, libc::SO_RCVBUF)?;
        let granted_sndbuf = socket.get_int(libc::SOL_SOCKET, libc::SO_SNDBUF)?;
        Ok(Self {
            granted_rcvbuf,
            granted_sndbuf,
            ..socket
        })
    }

    fn configure(&self) -> io::Result<()> {
        self.set_int(libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 0)?;
        self.set_int(libc::SOL_SOCKET, libc::SO_RCVBUF, WANT_RCVBUF)?;
        self.set_int(libc::SOL_SOCKET, libc::SO_SNDBUF, WANT_SNDBUF)?;

        // Reports v4 arrivals too, as `::ffff:a.b.c.d`. The v4-level options
        // (`IP_RECVDSTADDR`, `IP_TOS`) are refused on this socket, so v4
        // sends carry their traffic class per datagram; see `send_msg`.
        let _ = self.set_int(libc::IPPROTO_IPV6, libc::IPV6_RECVPKTINFO, 1);
        let _ = self.set_int(libc::IPPROTO_IPV6, libc::IPV6_TCLASS, DSCP_EF);

        // Do not fragment, as on Linux. `IP_DONTFRAG` is refused on this
        // socket; `IPV6_DONTFRAG` covers v4-mapped sends as well.
        self.set_int(libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG, 1)?;

        self.set_ttl(DEFAULT_TTL)?;
        self.set_nonblocking()?;
        Ok(())
    }

    /// Bind `[::]` at `port`, or at a kernel-chosen one for 0, with the IPv4
    /// side of that port free.
    ///
    /// macOS lets a dual-stack bind share a port another socket holds for
    /// `0.0.0.0`, and that socket then keeps every v4 datagram. So an IPv4
    /// socket takes the port first: it is refused where the port is held for
    /// IPv4, and while it holds the port nobody else can take that side. Once
    /// the dual-stack socket is bound, a v4 bind to the port is refused, so
    /// closing the guard leaves no gap.
    fn bind(&self, port: u16) -> io::Result<()> {
        // A kernel-chosen v4 port can still be held for IPv6 alone; choose
        // again rather than fail an open that asked for any port.
        let attempts = if port == 0 { 8 } else { 1 };
        let mut last = None;
        for _ in 0..attempts {
            let (_guard, chosen) = hold_v4(port)?;
            match self.bind_v6(chosen) {
                Ok(()) => return Ok(()),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| io::ErrorKind::AddrInUse.into()))
    }

    fn bind_v6(&self, port: u16) -> io::Result<()> {
        let addr = libc::sockaddr_in6 {
            sin6_len: mem::size_of::<libc::sockaddr_in6>() as u8,
            sin6_family: family_v6(),
            sin6_port: port.to_be(),
            sin6_flowinfo: 0,
            sin6_addr: libc::in6_addr { s6_addr: [0u8; 16] },
            sin6_scope_id: 0,
        };
        let rc = unsafe {
            libc::bind(
                self.fd.as_raw_fd(),
                core::ptr::addr_of!(addr).cast(),
                socklen(mem::size_of::<libc::sockaddr_in6>()),
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn granted_recv_buffer(&self) -> i32 {
        self.granted_rcvbuf
    }

    pub fn granted_send_buffer(&self) -> i32 {
        self.granted_sndbuf
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let mut len = socklen(mem::size_of::<libc::sockaddr_storage>());
        let rc = unsafe {
            libc::getsockname(
                self.fd.as_raw_fd(),
                core::ptr::addr_of_mut!(storage).cast(),
                &mut len,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        from_storage(&storage).ok_or_else(|| io::Error::other("unrecognised local address"))
    }

    /// `IP_TTL` is refused on this socket; the v6 hop limit carries to
    /// v4-mapped sends.
    pub fn set_ttl(&self, ttl: u8) -> io::Result<()> {
        self.set_int(libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS, i32::from(ttl))
    }

    pub fn ttl(&self) -> io::Result<u8> {
        let value = self.get_int(libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS)?;
        u8::try_from(value).map_err(|_| io::Error::other("ttl outside the byte range"))
    }

    fn set_nonblocking(&self) -> io::Result<()> {
        let flags = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let rc = unsafe {
            libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        let _ = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        Ok(())
    }

    fn set_int(&self, level: libc::c_int, name: libc::c_int, value: libc::c_int) -> io::Result<()> {
        let rc = unsafe {
            libc::setsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                core::ptr::addr_of!(value).cast(),
                socklen(mem::size_of::<libc::c_int>()),
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn get_int(&self, level: libc::c_int, name: libc::c_int) -> io::Result<i32> {
        let mut value: libc::c_int = 0;
        let mut len = socklen(mem::size_of::<libc::c_int>());
        let rc = unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                core::ptr::addr_of_mut!(value).cast(),
                &mut len,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }

    pub fn send_to(&self, datagram: &[u8], to: SocketAddr) -> io::Result<usize> {
        self.send_msg(datagram, to, None)
    }

    /// One datagram to `to`, claiming `source` when one is pinned.
    ///
    /// A v4-mapped destination takes the kernel's v4 path, which ignores the
    /// socket's `IPV6_TCLASS` and reads only `IPPROTO_IP` control messages, so
    /// a v4 send carries `IP_TOS` itself and a v4 source rides `IP_PKTINFO`.
    /// A v6 destination keeps the socket's traffic class.
    pub(super) fn send_msg(
        &self,
        datagram: &[u8],
        to: SocketAddr,
        source: Option<IpAddr>,
    ) -> io::Result<usize> {
        let (addr, addr_len) = to_storage(to);
        let mark = to.is_ipv4();

        let mut control = Control([0u8; CONTROL_LEN]);
        let mut iov = libc::iovec {
            iov_base: datagram.as_ptr().cast_mut().cast(),
            iov_len: datagram.len(),
        };
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = core::ptr::addr_of!(addr).cast_mut().cast();
        msg.msg_namelen = addr_len;
        msg.msg_iov = core::ptr::addr_of_mut!(iov);
        msg.msg_iovlen = 1;
        let mut control_len = 0;
        if mark {
            control_len += unsafe { libc::CMSG_SPACE(TOS_FIELD) };
        }
        if let Some(source) = source {
            control_len += unsafe { libc::CMSG_SPACE(source_field(source)) };
        }
        if control_len > 0 {
            msg.msg_control = control.0.as_mut_ptr().cast();
            msg.msg_controllen = control_len;
        }

        // SAFETY: the control buffer is aligned and msg_controllen reserves
        // CMSG_SPACE for exactly the messages written below.
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            if mark {
                (*cmsg).cmsg_level = libc::IPPROTO_IP;
                (*cmsg).cmsg_type = libc::IP_TOS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(TOS_FIELD);
                core::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>(), DSCP_EF);
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
            if let Some(source) = source {
                if cmsg.is_null() {
                    return Err(io::Error::other("no room for the source"));
                }
                write_source(cmsg, source);
            }
        }

        let sent = unsafe { libc::sendmsg(self.fd.as_raw_fd(), &msg, 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(usize::try_from(sent).unwrap_or(0))
    }

    pub fn recv_from(
        &self,
        buf: &mut [u8],
        wait: Duration,
    ) -> io::Result<Option<(usize, SocketAddr)>> {
        let mut ready = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
        let rc = unsafe { libc::poll(&raw mut ready, 1, ms) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::Interrupted => Ok(None),
                _ => Err(error),
            };
        }
        if rc == 0 {
            return Ok(None);
        }
        let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let mut len = socklen(mem::size_of::<libc::sockaddr_storage>());
        let got = unsafe {
            libc::recvfrom(
                self.fd.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                core::ptr::addr_of_mut!(storage).cast(),
                &mut len,
            )
        };
        if got < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(None),
                _ => Err(error),
            };
        }
        let from = from_storage(&storage)
            .ok_or_else(|| io::Error::other("unrecognised source address"))?;
        Ok(Some((usize::try_from(got).unwrap_or(0), from)))
    }

    pub(super) fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// An IPv4 socket bound at `0.0.0.0:port`, and the port it got.
fn hold_v4(port: u16) -> io::Result<(OwnedFd, u16)> {
    let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
    addr.sin_len = mem::size_of::<libc::sockaddr_in>() as u8;
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_port = port.to_be();
    let mut len = socklen(mem::size_of::<libc::sockaddr_in>());
    let rc = unsafe { libc::bind(raw, core::ptr::addr_of!(addr).cast(), len) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::getsockname(raw, core::ptr::addr_of_mut!(addr).cast(), &mut len) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fd, u16::from_be(addr.sin_port)))
}

pub(super) fn to_storage(addr: SocketAddr) -> (libc::sockaddr_in6, libc::socklen_t) {
    let (ip, port) = match addr {
        SocketAddr::V4(v4) => (v4.ip().to_ipv6_mapped(), v4.port()),
        SocketAddr::V6(v6) => (*v6.ip(), v6.port()),
    };
    let storage = libc::sockaddr_in6 {
        sin6_len: mem::size_of::<libc::sockaddr_in6>() as u8,
        sin6_family: family_v6(),
        sin6_port: port.to_be(),
        sin6_flowinfo: 0,
        sin6_addr: libc::in6_addr {
            s6_addr: ip.octets(),
        },
        sin6_scope_id: 0,
    };
    (storage, socklen(mem::size_of::<libc::sockaddr_in6>()))
}

pub(super) fn from_storage(storage: &libc::sockaddr_storage) -> Option<SocketAddr> {
    match libc::c_int::from(storage.ss_family) {
        libc::AF_INET6 => {
            let v6 = unsafe { &*core::ptr::from_ref(storage).cast::<libc::sockaddr_in6>() };
            let ip = Ipv6Addr::from(v6.sin6_addr.s6_addr);
            let port = u16::from_be(v6.sin6_port);
            Some(match ip.to_ipv4_mapped() {
                Some(v4) => SocketAddr::V4(SocketAddrV4::new(v4, port)),
                None => SocketAddr::V6(SocketAddrV6::new(
                    ip,
                    port,
                    v6.sin6_flowinfo,
                    v6.sin6_scope_id,
                )),
            })
        }
        libc::AF_INET => {
            let v4 = unsafe { &*core::ptr::from_ref(storage).cast::<libc::sockaddr_in>() };
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))),
                u16::from_be(v4.sin_port),
            ))
        }
        _ => None,
    }
}

/// Room for one datagram's control messages: the traffic class beside a v6
/// packet information is 16 plus 32 bytes of aligned space.
pub(super) const CONTROL_LEN: usize = 64;

/// Control message storage, aligned as the kernel requires.
#[repr(align(8))]
#[derive(Clone, Copy)]
pub(super) struct Control(pub(super) [u8; CONTROL_LEN]);

const TOS_FIELD: u32 = 4;
const _: () = assert!(TOS_FIELD as usize == mem::size_of::<libc::c_int>());
const V4_SOURCE_FIELD: u32 = 12;
const _: () = assert!(V4_SOURCE_FIELD as usize == mem::size_of::<libc::in_pktinfo>());
const V6_SOURCE_FIELD: u32 = 20;
const _: () = assert!(V6_SOURCE_FIELD as usize == mem::size_of::<libc::in6_pktinfo>());

fn source_field(source: IpAddr) -> u32 {
    match source {
        IpAddr::V4(_) => V4_SOURCE_FIELD,
        IpAddr::V6(_) => V6_SOURCE_FIELD,
    }
}

/// Fill one control message with the source address.
///
/// The v4 source goes into `ipi_spec_dst` at `IPPROTO_IP`: a v4-mapped
/// destination takes the v4 path, which ignores `IPV6_PKTINFO` and leaves
/// from the kernel's own choice. Interface indexes stay zero so routing still
/// picks the interface; only the address is claimed.
///
/// # Safety
///
/// `cmsg` must point into a control buffer with `CMSG_SPACE` room for the
/// structure the source's family selects.
unsafe fn write_source(cmsg: *mut libc::cmsghdr, source: IpAddr) {
    match source {
        IpAddr::V4(v4) => unsafe {
            (*cmsg).cmsg_level = libc::IPPROTO_IP;
            (*cmsg).cmsg_type = libc::IP_PKTINFO;
            (*cmsg).cmsg_len = libc::CMSG_LEN(V4_SOURCE_FIELD);
            let mut info = mem::zeroed::<libc::in_pktinfo>();
            info.ipi_spec_dst.s_addr = u32::from(v4).to_be();
            core::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::in_pktinfo>(), info);
        },
        IpAddr::V6(v6) => unsafe {
            (*cmsg).cmsg_level = libc::IPPROTO_IPV6;
            (*cmsg).cmsg_type = libc::IPV6_PKTINFO;
            (*cmsg).cmsg_len = libc::CMSG_LEN(V6_SOURCE_FIELD);
            let mut info = mem::zeroed::<libc::in6_pktinfo>();
            info.ipi6_addr.s6_addr = v6.octets();
            core::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::in6_pktinfo>(), info);
        },
    }
}

/// The local address a datagram arrived at, from its `IPV6_PKTINFO`. A v4
/// arrival is reported v4-mapped there and returned as v4.
pub(super) fn local_of(msg: &libc::msghdr) -> Option<IpAddr> {
    // SAFETY: the control pointer and length name storage the caller owns,
    // sized by the kernel to what it wrote; the reads are unaligned-tolerant.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::IPPROTO_IPV6 && (*cmsg).cmsg_type == libc::IPV6_PKTINFO {
                let info =
                    core::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::in6_pktinfo>());
                let ip = Ipv6Addr::from(info.ipi6_addr.s6_addr);
                return Some(match ip.to_ipv4_mapped() {
                    Some(v4) => IpAddr::V4(v4),
                    None => IpAddr::V6(ip),
                });
            }
            cmsg = libc::CMSG_NXTHDR(msg, cmsg);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TOS byte a v4 datagram arrived with, read by a plain `AF_INET`
    /// socket, which is what a v4 peer is.
    fn arrived_tos(receiver: &OwnedFd) -> u8 {
        let mut ready = libc::pollfd {
            fd: receiver.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&raw mut ready, 1, 1000) }, 1, "nothing arrived");
        let mut buf = [0u8; 64];
        let mut control = Control([0u8; CONTROL_LEN]);
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.0.as_mut_ptr().cast();
        msg.msg_controllen = socklen(CONTROL_LEN);
        assert!(unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut msg, 0) } > 0);
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            assert!(!cmsg.is_null(), "no TOS reported");
            assert_eq!(((*cmsg).cmsg_level, (*cmsg).cmsg_type), (libc::IPPROTO_IP, libc::IP_RECVTOS));
            *libc::CMSG_DATA(cmsg)
        }
    }

    /// A kernel-chosen port is never one another socket holds for IPv4.
    /// A plain dual-stack bind picks blind across both families: with 200 of
    /// 16384 ephemeral ports held, 500 binds share one about six times, so
    /// this fails all but about 0.2 % of runs without the guard. The 200 stay
    /// under the 256-descriptor default.
    #[test]
    fn a_chosen_port_is_never_held_for_ipv4() {
        let held: Vec<_> = (0..200)
            .map(|_| std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("hold"))
            .collect();
        let ports: std::collections::HashSet<u16> = held
            .iter()
            .map(|socket| socket.local_addr().expect("addr").port())
            .collect();
        for _ in 0..500 {
            let port = Socket::bound(0).expect("bind").local_addr().expect("addr").port();
            assert!(!ports.contains(&port), "took {port}, which is held for IPv4");
        }
    }

    /// A v4 datagram past the loopback MTU (16384) is refused rather than
    /// fragmented, so a path probe cannot read pieces as success.
    #[test]
    fn an_oversized_v4_send_is_refused() {
        let sender = Socket::bound(0).expect("sender");
        let receiver = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("receiver");
        let to = receiver.local_addr().expect("addr");
        let error = sender.send_to(&[0u8; 20_000], to).expect_err("fragmented");
        assert_eq!(error.raw_os_error(), Some(libc::EMSGSIZE));
    }

    /// v4 sends leave marked EF, pinned or not. The socket's own `IP_TOS` is
    /// refused here and its `IPV6_TCLASS` reaches v6 sends only, so without
    /// the per-datagram mark these arrive at 0.
    #[test]
    fn a_v4_send_leaves_marked() {
        let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        assert!(raw >= 0);
        let receiver = unsafe { OwnedFd::from_raw_fd(raw) };
        let on: libc::c_int = 1;
        let rc = unsafe {
            libc::setsockopt(
                raw,
                libc::IPPROTO_IP,
                libc::IP_RECVTOS,
                (&raw const on).cast(),
                socklen(mem::size_of::<libc::c_int>()),
            )
        };
        assert_eq!(rc, 0);
        let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
        addr.sin_len = mem::size_of::<libc::sockaddr_in>() as u8;
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
        let rc = unsafe {
            libc::bind(raw, (&raw const addr).cast(), socklen(mem::size_of::<libc::sockaddr_in>()))
        };
        assert_eq!(rc, 0);
        let mut len = socklen(mem::size_of::<libc::sockaddr_in>());
        assert_eq!(unsafe { libc::getsockname(raw, (&raw mut addr).cast(), &mut len) }, 0);
        let to = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), u16::from_be(addr.sin_port));

        let sender = Socket::bound(0).expect("sender");
        sender.send_to(b"free", to).expect("send");
        assert_eq!(arrived_tos(&receiver), DSCP_EF as u8, "an unpinned v4 send left unmarked");
        sender
            .send_msg(b"pin", to, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)))
            .expect("send");
        assert_eq!(arrived_tos(&receiver), DSCP_EF as u8, "a pinned v4 send left unmarked");
    }
}
