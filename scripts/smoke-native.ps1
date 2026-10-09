param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
$taskExecutable = (Resolve-Path -LiteralPath $Executable).Path
# Exercise the actual normal executable, not a hidden diagnostic entry point.
# Only a fresh, isolated profile is supplied. No account/clipboard interaction.
$taskProfile = Join-Path ([IO.Path]::GetTempPath()) ('parsec-native-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskProfile -Force | Out-Null
$taskDevice = $null
foreach ($taskLaunch in 1..2) {
    $taskInfo = [Diagnostics.ProcessStartInfo]::new($taskExecutable)
    $taskInfo.UseShellExecute = $false
    # This launches the client UI under test, not a background helper/service.
    $taskInfo.WindowStyle = [Diagnostics.ProcessWindowStyle]::Normal
    $taskInfo.Environment['LOCALAPPDATA'] = $taskProfile
    $taskProcess = [Diagnostics.Process]::Start($taskInfo)
    try {
        $taskUntil = [DateTime]::UtcNow.AddSeconds(180)
        do {
            Start-Sleep -Milliseconds 250
            $taskProcess.Refresh()
            if ($taskProcess.HasExited) { throw "Normal client exited before window startup: $($taskProcess.ExitCode)" }
        } until ($taskProcess.MainWindowHandle -ne [IntPtr]::Zero -or [DateTime]::UtcNow -gt $taskUntil)
        if ($taskProcess.MainWindowHandle -eq [IntPtr]::Zero) { throw 'Normal native window did not appear' }
        Start-Sleep -Seconds 5
        $taskProcess.Refresh()
        if ($taskProcess.MainWindowTitle -ne 'ParsecWebTurn') { throw 'Guest overwrote the application identity' }
        if (!$taskProcess.CloseMainWindow()) { throw 'Native window did not accept close request' }
        if (!$taskProcess.WaitForExit(20000)) { throw 'Normal client did not finish shutdown' }
        if ($taskProcess.ExitCode -ne 0) { throw "Normal client shutdown failed: $($taskProcess.ExitCode)" }
        $taskSaved = Join-Path $taskProfile 'ParsecWebTurn/Native/profile.dpapi'
        if (!(Test-Path -LiteralPath $taskSaved) -or (Get-Item -LiteralPath $taskSaved).Length -eq 0) { throw 'Normal native profile was not saved' }
        # Verify real guest writes, not just the existence of an encrypted envelope.
        # This profile is isolated and contains no real account credentials.
        $taskPlain = [Security.Cryptography.ProtectedData]::Unprotect([IO.File]::ReadAllBytes($taskSaved), $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
        $taskData = [Text.Encoding]::UTF8.GetString($taskPlain) | ConvertFrom-Json
        $taskCurrentDevice = $taskData.files.'/.parsec-persistent/devid.bin'
        if (!$taskCurrentDevice -or $taskCurrentDevice.Count -eq 0) { throw 'Original guest did not persist its device identity' }
        if (!$taskData.files.'/config.json' -or $taskData.files.'/config.json'.Count -eq 0) { throw 'Original guest did not persist client settings' }
        $taskIdentity = ($taskCurrentDevice -join ',')
        if ($taskLaunch -eq 2 -and $taskIdentity -ne $taskDevice) { throw 'Original guest device identity changed after restart' }
        $taskDevice = $taskIdentity
    } finally {
        if (!$taskProcess.HasExited) { $taskProcess.Kill(); $taskProcess.WaitForExit() }
        $taskProcess.Dispose()
    }
}
Write-Output 'Normal executable startup, close, encrypted save and restart succeeded.'

