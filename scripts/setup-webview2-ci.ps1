$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This dependency setup is for disposable GitHub CI runners only.' }

function Test-WebView2Runtime {
    $appId = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($key in @("HKLM:/SOFTWARE/WOW6432Node/Microsoft/EdgeUpdate/Clients/$appId", "HKLM:/SOFTWARE/Microsoft/EdgeUpdate/Clients/$appId", "HKCU:/SOFTWARE/Microsoft/EdgeUpdate/Clients/$appId")) {
        $value = Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue
        if ($value.pv -and $value.pv -ne '0.0.0.0') { return $true }
    }
    return $false
}

if (Test-WebView2Runtime) { Write-Output 'WebView2 Runtime is already installed.'; exit 0 }
$installer = Join-Path $env:RUNNER_TEMP 'MicrosoftEdgeWebview2Setup.exe'
Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $installer
$process = Start-Process -FilePath $installer -ArgumentList '/silent', '/install' -WindowStyle Hidden -Wait -PassThru
# The bootstrapper may return before its installer child has finished.
for ($attempt = 0; $attempt -lt 120; $attempt++) {
    if (Test-WebView2Runtime) { Write-Output 'WebView2 Runtime installed for CI validation.'; exit 0 }
    Start-Sleep -Seconds 1
}
throw "WebView2 Runtime did not become available (bootstrapper exit $($process.ExitCode))."
