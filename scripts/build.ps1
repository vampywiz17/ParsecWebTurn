param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    $version = (Get-Content -LiteralPath VERSION -Raw).Trim()
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid VERSION' }
    $config = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
    $cargoManifest = Get-Content src-tauri/Cargo.toml -Raw
    if ($config.version -ne $version -or $cargoManifest -notmatch "(?m)^version = `"$([regex]::Escape($version))`"$") { throw 'VERSION, Tauri and Cargo versions must match' }
    function Invoke-Cargo {
        & cargo @args
        if ($LASTEXITCODE -ne 0) { throw "Cargo command failed: $args" }
    }
    Invoke-Cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
    & node --test tests/injection.test.cjs tests/stats.test.cjs tests/window.test.cjs
    if ($LASTEXITCODE -ne 0) { throw 'Injection tests failed' }
    Invoke-Cargo test --locked --manifest-path src-tauri/Cargo.toml
    # Pass the -- separator directly to the native command; PowerShell
    # consumes it when forwarding through a script function's $args.
    & cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    Invoke-Cargo build --locked --release --manifest-path src-tauri/Cargo.toml
    Copy-Item -LiteralPath src-tauri/target/release/parsec-web-turn.exe -Destination ParsecWebTurn.exe -Force

    # A new staging directory and an explicit allowlist keep local credentials,
    # profiles and credentials out of every artifact. Web assets are embedded.
    $staging = Join-Path $repoRoot ('dist/staging-' + [guid]::NewGuid().ToString('N'))
    $packageName = "ParsecWebTurn-v$version-win64"
    $package = Join-Path $staging $packageName
    New-Item -ItemType Directory -Path $package -Force | Out-Null
    foreach ($file in @('ParsecWebTurn.exe', 'ice.example.json', 'README.md', 'LICENSE', 'CHANGELOG.md')) {
        Copy-Item -LiteralPath $file -Destination $package
    }
    $zip = "dist/$packageName.zip"
    Compress-Archive -LiteralPath $package -DestinationPath $zip -Force
    @(
        "$((Get-FileHash ParsecWebTurn.exe -Algorithm SHA256).Hash.ToLower())  ParsecWebTurn.exe"
        "$((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower())  $packageName.zip"
    ) | Set-Content -LiteralPath dist/SHA256SUMS.txt -Encoding ascii
} finally {
    Pop-Location
}
