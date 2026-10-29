# lowlat-common for macOS

MIT source from nomi-san/lowlat revision
5f8db5689e55de0d396b0520824caa9fc053dd86, `crates/common`.
Original copyright and license: `../lowlat-client/LICENSE`.

Vendored only to build on macOS: `clock.rs` uses `clock_nanosleep`, which macOS
does not provide, so the absolute-deadline sleep there takes the existing
portable fallback on macOS. Workspace-inherited manifest fields are written out
and the `loom` cfg is declared. No other source changes.
