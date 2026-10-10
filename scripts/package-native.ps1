param()
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
Push-Location $taskRoot
try {
    $taskVersion = (Get-Content VERSION -Raw).Trim()
    if ($taskVersion -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid VERSION' }
    $taskManifestVersion = [regex]::Match((Get-Content src-native/Cargo.toml -Raw), '(?m)^version = "([^"]+)"\r?$').Groups[1].Value
    if ($taskManifestVersion -ne $taskVersion) { throw 'Native version mismatch' }
    $taskBinary = [IO.File]::ReadAllBytes((Join-Path $taskRoot 'src-native/target/x86_64-pc-windows-msvc/release/parsec-web-turn.exe'))
    $taskPeOffset = [BitConverter]::ToInt32($taskBinary, 0x3c)
    if ([BitConverter]::ToUInt16($taskBinary, $taskPeOffset + 24 + 68) -ne 2) { throw 'Only the normal Windows GUI executable may be packaged' }
    # Read the documented PE32+ import table, rather than searching incidental strings.
    $taskSectionTable = $taskPeOffset + 24 + [BitConverter]::ToUInt16($taskBinary, $taskPeOffset + 20)
    $taskSectionCount = [BitConverter]::ToUInt16($taskBinary, $taskPeOffset + 6)
    function Convert-TaskRva([uint32]$TaskRva) {
        for ($taskIndex = 0; $taskIndex -lt $taskSectionCount; $taskIndex++) {
            $taskSection = $taskSectionTable + 40 * $taskIndex
            $taskStart = [BitConverter]::ToUInt32($taskBinary, $taskSection + 12)
            $taskSize = [Math]::Max([BitConverter]::ToUInt32($taskBinary, $taskSection + 8), [BitConverter]::ToUInt32($taskBinary, $taskSection + 16))
            if ($TaskRva -ge $taskStart -and $TaskRva -lt $taskStart + $taskSize) {
                return [int]([BitConverter]::ToUInt32($taskBinary, $taskSection + 20) + $TaskRva - $taskStart)
            }
        }
        throw 'Invalid executable import address'
    }
    if ([BitConverter]::ToUInt16($taskBinary, $taskPeOffset + 24) -ne 0x20b) { throw 'Expected a Windows x64 executable' }
    $taskImport = Convert-TaskRva ([BitConverter]::ToUInt32($taskBinary, $taskPeOffset + 24 + 112 + 8))
    while (($taskNameRva = [BitConverter]::ToUInt32($taskBinary, $taskImport + 12)) -ne 0) {
        $taskNameOffset = Convert-TaskRva $taskNameRva
        $taskNameEnd = $taskNameOffset
        while ($taskBinary[$taskNameEnd] -ne 0) { $taskNameEnd++ }
        $taskDll = [Text.Encoding]::ASCII.GetString($taskBinary, $taskNameOffset, $taskNameEnd - $taskNameOffset)
        if ($taskDll -match '^(vcruntime|msvcp|libopus|WebView2Loader).*\.dll$') { throw "External application runtime dependency: $taskDll" }
        $taskImport += 20
    }
    $taskBinaryText = [Text.Encoding]::Latin1.GetString($taskBinary)
    foreach ($taskMarker in @('video-hardware-probe', 'login-audit', 'synthetic-session-value', 'private-password-token-url')) {
        if ($taskBinaryText.Contains($taskMarker)) { throw "Diagnostic marker in normal executable: $taskMarker" }
    }
    $taskFixture = [IO.File]::ReadAllBytes((Join-Path $taskRoot 'src-native/fixtures/synthetic-1920x1080.h264'))
    if ($taskBinaryText.Contains([Text.Encoding]::Latin1.GetString($taskFixture))) { throw 'Synthetic video fixture in normal executable' }
    $taskCore = [IO.File]::ReadAllBytes((Join-Path $taskRoot 'src-native/vendor/parsecd.wasm'))
    if (!$taskBinaryText.Contains([Text.Encoding]::Latin1.GetString($taskCore))) { throw 'Pinned WASM is not embedded in the executable' }
    Copy-Item src-native/target/x86_64-pc-windows-msvc/release/parsec-web-turn.exe ParsecWebTurn.exe -Force
    $taskStaging = Join-Path $taskRoot ('dist/native-package-' + [guid]::NewGuid().ToString('N'))
    $taskPackageName = "ParsecWebTurn-v$taskVersion-win64"
    $taskPackage = Join-Path $taskStaging $taskPackageName
    New-Item -ItemType Directory -Path $taskPackage -Force | Out-Null
    foreach ($taskFile in @('ParsecWebTurn.exe','README.md','LICENSE','CHANGELOG.md')) { Copy-Item -LiteralPath $taskFile -Destination $taskPackage }
    Copy-Item src-native/vendor/webrtc-0.14.0/LICENSE-MIT (Join-Path $taskPackage 'WEBRTC-LICENSE-MIT.txt')
    Copy-Item src-native/vendor/webrtc-0.14.0/LICENSE-APACHE (Join-Path $taskPackage 'WEBRTC-LICENSE-APACHE.txt')
    Copy-Item src-native/vendor/webrtc-0.14.0/LOCAL-CHANGES.md (Join-Path $taskPackage 'WEBRTC-LOCAL-CHANGES.md')
    Copy-Item src-native/vendor/webrtc-ice-0.14.0/LOCAL-CHANGES.md (Join-Path $taskPackage 'ICE-LOCAL-CHANGES.md')
    Copy-Item src-native/vendor/turn-0.11.0/LOCAL-CHANGES.md (Join-Path $taskPackage 'TURN-LOCAL-CHANGES.md')
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
