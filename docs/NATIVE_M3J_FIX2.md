# M3j / 0.13.2: preserve the pinned web CryptoHash stub

The user's 0.13.1 account report reached ten concurrently active workers and
rejected zero spawns. After login the rendering worker stopped at the explicit
unsupported-import boundary `env::MTY_CryptoHash`, not WASI proc_exit.

The pinned original `matoya-worker.js` defines MTY_CryptoHash with an empty
function body. The pinned WASM signature has seven i32 parameters and no
return value. The host now mirrors that exact unavailability: no hashing,
no output writes, no claimed successful digest, and no fatal missing-import
trap. This is compatibility with a private, version-pinned Matoya interface,
not a new cryptographic implementation or a web-standard API.

A controlled WASM regression invokes the actual host bridge with the exact
signature and checks that guest input, key and output memory are unchanged,
the call is recorded, and no missing-import boundary is reported. No account,
credentials or external requests are involved. Unknown imports still trap.

The original core and account network policy are unchanged. Actual post-login
behavior still needs the user's retest; a later unsupported import may be
reached. User reports and credentials are not committed or packaged.
