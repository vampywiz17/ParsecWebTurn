//! The system calls under the shell, on macOS: socket and options,
//! poll wait, pipe wake, interface walk, and message send/receive.

mod addrs;
mod io;
mod send;
mod socket;
#[cfg(feature = "turn-stream")]
mod tcp;
mod wake;

pub(crate) use addrs::interface_v4;
pub(crate) use io::Io;
pub(crate) use send::{offload_send, offload_unsupported, pinned_send};
pub use socket::Socket;
#[cfg(feature = "turn-stream")]
pub(crate) use tcp::Tcp;
pub(crate) use wake::{Wake, WakeHandle};
