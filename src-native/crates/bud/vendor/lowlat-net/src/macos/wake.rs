//! Application send wake on macOS using a non-blocking pipe.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

#[derive(Debug)]
pub(crate) struct Wake {
    read_fd: OwnedFd,
    write_fd: OwnedFd,
}

#[derive(Debug)]
pub(crate) struct WakeHandle {
    write_fd: OwnedFd,
}

impl Wake {
    pub(crate) fn new() -> io::Result<Self> {
        let mut fds = [0 as libc::c_int; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        for &fd in &fds {
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        Ok(Self {
            read_fd: unsafe { OwnedFd::from_raw_fd(fds[0]) },
            write_fd: unsafe { OwnedFd::from_raw_fd(fds[1]) },
        })
    }

    pub(crate) fn handle(&self) -> io::Result<WakeHandle> {
        Ok(WakeHandle {
            write_fd: self.write_fd.try_clone()?,
        })
    }

    pub(crate) fn take(&self) -> io::Result<bool> {
        let mut buf = [0u8; 512];
        let mut total = 0;
        loop {
            let rc = unsafe {
                libc::read(self.read_fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
            };
            if rc <= 0 {
                break;
            }
            total += rc;
        }
        Ok(total > 0)
    }

    pub(crate) fn raw(&self) -> RawFd {
        self.read_fd.as_raw_fd()
    }
}

impl WakeHandle {
    pub(crate) fn notify(&self) -> io::Result<()> {
        let token = [1u8];
        let rc = unsafe {
            libc::write(self.write_fd.as_raw_fd(), token.as_ptr().cast(), 1)
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(err);
        }
        Ok(())
    }
}
