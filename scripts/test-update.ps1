# Exercise EXE replacement/restart in an isolated directory, never the real app.
param([Parameter(Mandatory)][string]$ExecutablePath, [Parameter(Mandatory)][string]$SavedProfile)
$ErrorActionPreference = 'Stop'
$env:PARSECWEBTURN_NO_UPDATE_CHECK = '1'
$taskRoot = Join-Path (Split-Path $SavedProfile -Parent) ('update-smoke-' + [guid]::NewGuid().ToString('N'))
$taskStage = Join-Path $taskRoot '.parsec-update-test'
$taskData = Join-Path $taskRoot 'data'
New-Item -ItemType Directory -Path $taskStage, $taskData | Out-Null
$taskName = 'UpdateSmoke' + [guid]::NewGuid().ToString('N')
$taskTarget = Join-Path $taskRoot ($taskName + '.exe')
Copy-Item -LiteralPath $ExecutablePath -Destination $taskTarget
Copy-Item -LiteralPath (Join-Path $SavedProfile 'settings.json') -Destination (Join-Path $taskData 'settings.json')
$taskSettingsHash = (Get-FileHash (Join-Path $taskData 'settings.json')).Hash
$taskOriginalHash = (Get-FileHash $taskTarget).Hash
Copy-Item -LiteralPath $ExecutablePath -Destination (Join-Path $taskStage 'helper.exe')
$taskCandidate = Join-Path $taskStage 'update.exe'
# A harmless PE overlay distinguishes the replacement from the original EXE.
$taskCandidateBytes = [IO.File]::ReadAllBytes($ExecutablePath) + [Text.Encoding]::ASCII.GetBytes('portable-update-smoke')
[IO.File]::WriteAllBytes($taskCandidate, $taskCandidateBytes)
$taskCandidateHash = (Get-FileHash $taskCandidate).Hash.ToLowerInvariant()
$taskProcess = $null
$taskHelper = $null
$taskReplacement = $null
try {
    $taskStart = [Diagnostics.ProcessStartInfo]::new()
    $taskStart.FileName = $taskTarget
    $taskStart.Arguments = '--settings --data-dir "' + $taskData + '"'
    $taskStart.UseShellExecute = $false
    $taskStart.CreateNoWindow = $true
    $taskProcess = [Diagnostics.Process]::Start($taskStart)
    for ($taskIndex = 0; $taskIndex -lt 300; $taskIndex++) {
        $taskProcess.Refresh()
        if ($taskProcess.HasExited) { throw 'Original update-test app exited early' }
        if ($taskProcess.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 100
    }
    if ($taskProcess.MainWindowHandle -eq [IntPtr]::Zero) { throw 'Original app did not show settings' }
    $taskPlan = @{
        target_name = $taskName + '.exe'; root = $taskData; settings = $true
        parent_pid = $taskProcess.Id; digest = $taskCandidateHash
    } | ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $taskStage 'plan.json'), $taskPlan, [Text.UTF8Encoding]::new($false))
    $taskStart.FileName = Join-Path $taskStage 'helper.exe'
    $taskStart.Arguments = '--apply-update'
    $taskHelper = [Diagnostics.Process]::Start($taskStart)
    Start-Sleep -Milliseconds 300
    if ((Get-FileHash $taskTarget).Hash -ne $taskOriginalHash) { throw 'Updater replaced EXE before original app exited' }
    if (!$taskProcess.CloseMainWindow()) { throw 'Cannot close original update-test window' }
    if (!$taskProcess.WaitForExit(30000)) { throw 'Original app failed to exit' }
    if (!$taskHelper.WaitForExit(30000) -or $taskHelper.ExitCode -ne 0) { throw 'Update helper failed' }
    if ((Get-FileHash $taskTarget).Hash.ToLowerInvariant() -ne $taskCandidateHash) { throw 'Replacement digest mismatch' }
    if ((Get-FileHash (Join-Path $taskStage 'previous.exe')).Hash -ne $taskOriginalHash) { throw 'Recovery EXE missing' }
    if ((Get-FileHash (Join-Path $taskData 'settings.json')).Hash -ne $taskSettingsHash) { throw 'Updater modified settings' }
    for ($taskIndex = 0; $taskIndex -lt 300; $taskIndex++) {
        $taskReplacement = [Diagnostics.Process]::GetProcessesByName($taskName) | Where-Object { $_.Id -ne $taskProcess.Id } | Select-Object -First 1
        if ($taskReplacement) {
            $taskReplacement.Refresh()
            if ($taskReplacement.MainWindowHandle -ne [IntPtr]::Zero) { break }
        }
        Start-Sleep -Milliseconds 100
    }
    if (!$taskReplacement -or $taskReplacement.MainWindowHandle -eq [IntPtr]::Zero) { throw 'Updated app did not restart in settings mode' }
    if (!$taskReplacement.CloseMainWindow() -or !$taskReplacement.WaitForExit(30000)) { throw 'Updated app failed to close' }
    Write-Output 'PASS: helper waits for exit, swaps the EXE, keeps backup/settings and restarts the renamed app with data-dir/settings preserved'
} finally {
    foreach ($taskOwnedProcess in @($taskProcess, $taskHelper, $taskReplacement)) {
        if ($taskOwnedProcess -and !$taskOwnedProcess.HasExited) { $taskOwnedProcess.Kill(); $taskOwnedProcess.WaitForExit() }
    }
}
