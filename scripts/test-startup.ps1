# Native visibility checks for disposable smoke-test profiles only.
param([Parameter(Mandatory)][string]$ExecutablePath, [Parameter(Mandatory)][string]$SavedProfile)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class StartupWindows {
    private delegate bool Callback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(Callback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder title, int count);
    public static int Visible(uint process) {
        int result = 0;
        EnumWindows((window, data) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner != process || !IsWindowVisible(window)) return true;
            var title = new StringBuilder(512); GetWindowText(window, title, title.Capacity);
            if (title.ToString().StartsWith("ParsecWebTurn", StringComparison.Ordinal)) result |= 1;
            if (title.ToString().StartsWith("Parsec ", StringComparison.Ordinal)) result |= 2;
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@
function Assert-Startup([string]$Profile, [bool]$ForceSettings, [bool]$ExpectParsec) {
    $taskStart = [System.Diagnostics.ProcessStartInfo]::new()
    $taskStart.FileName = $ExecutablePath
    $taskStart.Arguments = '--data-dir "' + $Profile + '"'
    if ($ForceSettings) { $taskStart.Arguments += ' --settings' }
    $taskStart.UseShellExecute = $false
    $taskStart.CreateNoWindow = $true
    $taskProcess = [System.Diagnostics.Process]::Start($taskStart)
    try {
        $taskClock = [System.Diagnostics.Stopwatch]::StartNew()
        $taskSeenAt = $null
        while ($taskClock.Elapsed.TotalSeconds -lt 30) {
            if ($taskProcess.HasExited) { throw 'App exited before showing its startup window' }
            $taskVisible = [StartupWindows]::Visible([uint32]$taskProcess.Id)
            if ($ExpectParsec -and ($taskVisible -band 1)) { throw 'Settings flashed during automatic startup' }
            if (!$ExpectParsec -and ($taskVisible -band 2)) { throw 'Unexpected automatic connection' }
            $taskExpected = if ($ExpectParsec) { 2 } else { 1 }
            if ($taskVisible -band $taskExpected) {
                if ($null -eq $taskSeenAt) { $taskSeenAt = $taskClock.ElapsedMilliseconds }
                if ($taskClock.ElapsedMilliseconds - $taskSeenAt -ge 500) { return }
            }
            Start-Sleep -Milliseconds 10
        }
        throw 'Startup window did not appear'
    } finally {
        if (!$taskProcess.HasExited) { $taskProcess.Kill(); $taskProcess.WaitForExit() }
        $taskProcess.Dispose()
    }
}
Assert-Startup $SavedProfile $false $true
Assert-Startup $SavedProfile $true $false
$taskFreshProfile = Join-Path (Split-Path $SavedProfile -Parent) ('startup-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskFreshProfile | Out-Null
Assert-Startup $taskFreshProfile $false $false
Set-Content -LiteralPath (Join-Path $taskFreshProfile 'settings.json') -Value '{invalid'
Assert-Startup $taskFreshProfile $false $false
Write-Output 'PASS: native startup visibility, automatic connection without settings flash, --settings, first run and invalid settings'
