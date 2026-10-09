param()
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
Push-Location $taskRoot
try {
    $taskVersion = (Get-Content VERSION -Raw).Trim()
    if ($taskVersion -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid VERSION' }
    if ((Get-Content src-native/Cargo.toml -Raw) -notmatch "(?m)^version = `"$taskVersion`"$") { throw 'Native version mismatch' }
    Copy-Item src-native/target/release/parsec-web-turn.exe ParsecWebTurn.exe -Force
    $taskStaging = Join-Path $taskRoot ('dist/native-package-' + [guid]::NewGuid().ToString('N'))
    $taskPackageName = "ParsecWebTurn-v$taskVersion-win64"
    $taskPackage = Join-Path $taskStaging $taskPackageName
    New-Item -ItemType Directory -Path $taskPackage -Force | Out-Null
    foreach ($taskFile in @('ParsecWebTurn.exe','README.md','LICENSE','CHANGELOG.md')) { Copy-Item -LiteralPath $taskFile -Destination $taskPackage }
    Copy-Item src-native/vendor/webrtc-0.14.0/LICENSE-MIT (Join-Path $taskPackage 'WEBRTC-LICENSE-MIT.txt')
    Copy-Item src-native/vendor/webrtc-0.14.0/LICENSE-APACHE (Join-Path $taskPackage 'WEBRTC-LICENSE-APACHE.txt')
    Copy-Item src-native/vendor/webrtc-0.14.0/LOCAL-CHANGES.md (Join-Path $taskPackage 'WEBRTC-LOCAL-CHANGES.md')
    $taskCargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $taskOpus = Get-ChildItem -Path (Join-Path $taskCargoHome 'registry/src/*/libopus_sys-0.3.3') -Directory | Select-Object -First 1
    if (!$taskOpus) { throw 'Bundled Opus license directory missing' }
    Copy-Item (Join-Path $taskOpus.FullName 'LICENSE') (Join-Path $taskPackage 'LIBOPUS-SYS-LICENSE.txt')
    Copy-Item (Join-Path $taskOpus.FullName 'opus/COPYING') (Join-Path $taskPackage 'OPUS-LICENSE.txt')
    if ((Get-ChildItem -LiteralPath $taskPackage -Filter '*.exe').Count -ne 1 -or (Get-ChildItem -LiteralPath $taskPackage -Filter '*.wasm').Count -ne 0) { throw 'Single executable package violated' }
    $taskZip = "dist/$taskPackageName.zip"
    Compress-Archive -LiteralPath $taskPackage -DestinationPath $taskZip -Force
    @(
        "$((Get-FileHash ParsecWebTurn.exe -Algorithm SHA256).Hash.ToLowerInvariant())  ParsecWebTurn.exe"
        "$((Get-FileHash $taskZip -Algorithm SHA256).Hash.ToLowerInvariant())  $taskPackageName.zip"
    ) | Set-Content dist/SHA256SUMS.txt -Encoding ascii
} finally { Pop-Location }
