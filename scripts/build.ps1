param()
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
Push-Location $taskRoot
try {
    ./src-native/fetch-core.ps1
    & cargo fmt --manifest-path src-native/Cargo.toml -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting failed' }
    & cargo test --locked --manifest-path src-native/Cargo.toml --features diagnostics
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    & cargo clippy --locked --manifest-path src-native/Cargo.toml --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    & cargo clippy --locked --manifest-path src-native/Cargo.toml --features diagnostics --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Diagnostic Clippy failed' }
    & cargo build --locked --release --manifest-path src-native/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    ./scripts/package-native.ps1
} finally { Pop-Location }
