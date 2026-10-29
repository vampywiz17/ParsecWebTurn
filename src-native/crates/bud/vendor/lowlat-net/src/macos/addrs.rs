//! The interface walk behind the host candidates on macOS.

use core::net::Ipv4Addr;

/// Every IPv4 address on an interface that is up, as the system lists them;
/// which of them to offer is decided above.
pub(crate) fn interface_v4() -> Vec<Ipv4Addr> {
    let mut list: *mut libc::ifaddrs = core::ptr::null_mut();
    if unsafe { libc::getifaddrs(&raw mut list) } != 0 {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut node = list;
    while !node.is_null() {
        let entry = unsafe { &*node };
        node = entry.ifa_next;

        if entry.ifa_addr.is_null() || entry.ifa_flags & (libc::IFF_UP as u32) == 0 {
            continue;
        }
        if i32::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        let sin = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        found.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
    }

    unsafe { libc::freeifaddrs(list) };
    found
}
