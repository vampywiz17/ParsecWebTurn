# M3k-fix22 / 0.14.22 — media bursts bypass the control queue

The user's fix21 report confirms 15 real-host video messages (410,910 bytes,
largest 88,841 bytes). The first failure is inbound-queue-full on audio at
5,127 ms, with ICE/DTLS connected. This confirms complete video reception beyond
the former callback limit and identifies the shared UI queue as this failure.
No raw report, credentials or media payload is committed.

Configured sessions now count/discard unavailable video/audio directly at the
transport receive boundary, before allocating queue payloads. They cannot consume
the control queue's 16-message / 4 MiB budget while the guest/UI is not polling.
Raw transport-only attempts retain bounded binary receipts for their caller.
Size validation, first-failure retention and control overflow failure remain.
Backend snapshots copy cumulative ingress counters, including on disconnect.

Regression tests cover 128 pairs of 1 MiB video and small audio receipts without
polling, constant queue memory, preserved queued control, and control overflow.
The encrypted guest-control probe receives a 128-message media burst with no
guest polling, then resumes guest metrics/control and keyboard transmission.
Existing large-message and 31-second session probes remain.

Video/audio decoding and remote-video presentation are still unavailable.
Real-host validation of this queue fix requires the new build's report.