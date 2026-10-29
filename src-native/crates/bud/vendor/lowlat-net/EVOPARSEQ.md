# Native UDP accounting seam

MIT source from nomi-san/lowlat feat/win-seams,
revision 5f8db5689e55de0d396b0520824caa9fc053dd86, crates/net/src.
Original copyright and license are retained in LICENSE.

This copy keeps native socket IO, batching/offload, wakeups, interface discovery
and the BUD shell with the local changes below. Unused browser/SCTP and standalone NAT-probe modules
and their dependencies are omitted. The normal WebRTC client does not use it.
The unused direct lowlat-crypto dependency is omitted; the client seam retains
the required cryptography. Unused direct dependencies are checked by a crate lint.

The initial data-path addition was byte accounting: Batch increments a plain u64
only after a successful OS send, by the existing buffer length (or datagram
length on the fallback path). Failed sends and empty flushes do not count.
Shell counts each received UDP payload before parsing/authentication. Successful
offload and ordinary sends have identical byte-count semantics.

The counters include encrypted BUD records, acknowledgements, retransmissions,
STUN and TURN framing/control on this UDP socket. They exclude IP/UDP headers
and the separate account HTTPS/WSS connection. OS send acceptance is not proof
of remote delivery. No extra syscalls, payload copies, locks or GPU operations
are introduced. The client publishes the existing cumulative counters once per
protocol pass. Real loopback byte checks run with offload allowed and disabled;
failed sends, empty flushes, receive accounting and direct/coturn propagation
are tested separately.

Optional turn-stream originally added the mio/rustls carrier, framing output,
wake dispatch and shell integration from lowlat PR #2 at
befd5a29c7a331111f11378c0b5623de788e9769. UDP batching/accounting is retained.
Stream counters measure TURN plaintext frames including padding, excluding TLS
and network headers. There is no additional SDK, browser transport or decoder.

TLS backpressure/deadline correction from lowlat commit
`d5d8d9ed4b1208f00de9fe8f90286919b15a0240` is applied on top (2026-10-17).
Pending output includes rustls ciphertext. No new core datagram is consumed
until the previous frame drains through both framing and TLS. The original
frame age survives plaintext acceptance and partial socket progress; poll waits
are capped by write deadlines, and TLS control records after idle get a fresh
deadline. The 32 KiB TLS cap is retained. Four upstream regression tests are
included; EvoParseq byte accounting and pending-connect handling are retained.

EvoParseq's native stream IO adaptation (2026-10-17) replaces the direct Mio
dependency with Linux poll/eventfd and Windows ConnectEx/WSARecv/WSASend on the
existing wake completion port. Rustls, framing, deadlines, ordering and UDP
remain unchanged. Windows owns one reusable 32 KiB network send buffer until its
overlapped completion; it is ciphertext for TLS, not a decoded-frame copy. A
pending overlapped send retains the carrier's pending state and original age.
Cancellation drains outstanding completions before releasing their storage.
Tests cover both Windows completion notification modes and cancellation.

Post-turn application output can be drained without a second wait/receive
pass. This reuses the existing carrier drain and shared output accounting;
TCP/TLS backpressure is unchanged. The real loopback regression checks
ordered delivery and byte/datagram accounting without an added sender wake.
Carrier acceptance is not proof of peer receipt. No new dependency is added.

This notice documents the application-owned changes used by EvoParseq.

The 2026-10-26 dependency refresh raises the webpki-roots minimum to 1.0.10
(RUSTSEC-2026-0381). Application and BUD lockfiles also select rustls-webpki
0.103.16 (RUSTSEC-2026-0382) and Tokio 1.53.3 (RUSTSEC-2026-0386).
Rustls remains 0.23.46 with the same providers and certificate policy.

## macOS backend

`src/macos` adds a macOS system layer beside `src/linux` and `src/windows`:
IPv4/IPv6 UDP sockets with DSCP EF, a `poll(2)` wait with a non-blocking pipe
wake, `getifaddrs` interface discovery, plain `sendmsg` without segmentation
offload, and the nonblocking TCP carrier for turn-stream. TCP/TLS QoS marking is
not applied on macOS; the shared guard reports it as not applied.

The dual-stack socket refuses the IPv4-level options (`IP_TOS`, `IP_PKTINFO`,
`IP_DONTFRAG`, `IP_TTL`). Each IPv4 send therefore carries `IP_TOS` as a control
message, and a pinned IPv4 source rides `IP_PKTINFO`; IPv6 sends keep the
socket's `IPV6_TCLASS`. `IPV6_DONTFRAG` and `IPV6_UNICAST_HOPS` cover both
families. Receives use one `recvmsg` per datagram and report the arrival address
from `IPV6_PKTINFO`; the batched `recvmsg_x` is not in the public SDK and is not
used. macOS lets a dual-stack bind share a port another socket holds for
`0.0.0.0`, which then keeps the IPv4 datagrams, so the bind first holds the port
with an IPv4 socket and closes it once `[::]` is bound.

The module set and public API are unchanged. Outside `src/macos`, `lib.rs`,
`addrs.rs` and `socket.rs` extend their platform conditions, `send.rs` starts
offload off on macOS, and `stream_io.rs` treats `EINPROGRESS` as a pending
connect there. The crate's tests run on macOS in CI; there only 127.0.0.1 is a
local loopback address, and the offload tests hold the per-datagram path.

