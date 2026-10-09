param([string]$Destination = (Join-Path $PSScriptRoot 'vendor/parsecd.wasm'))
$ErrorActionPreference = 'Stop'
$expected = 'd663dd96df477c65479fb93eb88756c7fcafff581cc93be563625cd195a4b4a6'
$parent = Split-Path -Parent $Destination
New-Item -ItemType Directory -Path $parent -Force | Out-Null
$temporary = Join-Path $parent ('download-' + [guid]::NewGuid().ToString('N') + '.wasm')
try {
    Invoke-WebRequest -Uri 'https://web.parsec.app/parsecd' -OutFile $temporary
    if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
        throw 'The public Parsec WASM changed. Audit the new ABI before updating the source pin.'
    }
    Move-Item -LiteralPath $temporary -Destination $Destination -Force
} finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
}

