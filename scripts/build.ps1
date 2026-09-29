param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    $version = (Get-Content -LiteralPath VERSION -Raw).Trim()
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid VERSION' }
    $env:GOOS = 'windows'
    $env:GOARCH = 'amd64'

    function Invoke-Go {
        & go @args
        if ($LASTEXITCODE -ne 0) { throw "Go command failed: $args" }
    }

    $unformatted = & gofmt -l src
    if ($LASTEXITCODE -ne 0 -or $unformatted) { throw "Run gofmt on the Go sources: $unformatted" }
    Invoke-Go mod verify
    Invoke-Go run github.com/akavel/rsrc@v0.10.2 -arch amd64 -manifest assets/app.manifest -ico assets/parsec.ico -o src/rsrc.syso
    Invoke-Go vet -mod=readonly ./src
    Invoke-Go test -mod=readonly ./src
    & node --test tests/injection.test.cjs
    if ($LASTEXITCODE -ne 0) { throw 'Injection tests failed' }
    Invoke-Go build -mod=readonly -trimpath -ldflags "-s -w -H=windowsgui -X main.version=$version" -o ParsecWebTurn.exe ./src

    # A new staging directory and an explicit allowlist keep local credentials,
    # profiles and generated inject.js out of every artifact.
    $staging = Join-Path $repoRoot ('dist/staging-' + [guid]::NewGuid().ToString('N'))
    $packageName = "ParsecWebTurn-v$version-win64"
    $package = Join-Path $staging $packageName
    New-Item -ItemType Directory -Path "$package/extension" -Force | Out-Null
    foreach ($file in @('ParsecWebTurn.exe', 'ice.example.json', 'README.md', 'LICENSE', 'CHANGELOG.md')) {
        Copy-Item -LiteralPath $file -Destination $package
    }
    Copy-Item -LiteralPath extension/manifest.json, extension/inject.template.js -Destination "$package/extension"
    $zip = "dist/$packageName.zip"
    Compress-Archive -LiteralPath $package -DestinationPath $zip -Force
    @(
        "$((Get-FileHash ParsecWebTurn.exe -Algorithm SHA256).Hash.ToLower())  ParsecWebTurn.exe"
        "$((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower())  $packageName.zip"
    ) | Set-Content -LiteralPath dist/SHA256SUMS.txt -Encoding ascii
} finally {
    Pop-Location
}
