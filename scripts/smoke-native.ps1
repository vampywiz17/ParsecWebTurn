param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
$taskExecutable = (Resolve-Path -LiteralPath $Executable).Path
# Exercise the actual normal executable, not a hidden diagnostic entry point.
# Only a fresh, isolated profile is supplied. No account/clipboard interaction.
$taskProfile = Join-Path ([IO.Path]::GetTempPath()) ('parsec-native-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskProfile -Force | Out-Null
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
        if (!$taskProcess.CloseMainWindow()) { throw 'Native window did not accept close request' }
        if (!$taskProcess.WaitForExit(20000)) { throw 'Normal client did not finish shutdown' }
        if ($taskProcess.ExitCode -ne 0) { throw "Normal client shutdown failed: $($taskProcess.ExitCode)" }
        $taskSaved = Join-Path $taskProfile 'ParsecWebTurn/Native/profile.dpapi'
        if (!(Test-Path -LiteralPath $taskSaved) -or (Get-Item -LiteralPath $taskSaved).Length -eq 0) { throw 'Normal native profile was not saved' }
    } finally {
        if (!$taskProcess.HasExited) { $taskProcess.Kill(); $taskProcess.WaitForExit() }
        $taskProcess.Dispose()
    }
}
Write-Output 'Normal executable startup, close, encrypted save and restart succeeded.'

