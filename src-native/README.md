# Native Rust client

The application crate lives here. See [the root README](../README.md) for supported features, persistent profile behavior, build instructions and limitations.

The normal build embeds the audited Parsec WASM core. Run `./src-native/fetch-core.ps1` from the repository root before compiling. A changed upstream hash fails closed and requires a new ABI audit.

`diagnostics` is an opt-in build feature for offline probes and prototype reports. It is excluded from the distributed executable. Test fixtures under `fixtures` are used only by tests and diagnostic builds. The pinned Parsec ABI is a private integration contract; Windows decoding/presentation and WebRTC transport use documented APIs and protocols.

The vendored WebRTC dependency and its narrowly scoped compatibility changes are documented in [LOCAL-CHANGES.md](vendor/webrtc-0.14.0/LOCAL-CHANGES.md).
