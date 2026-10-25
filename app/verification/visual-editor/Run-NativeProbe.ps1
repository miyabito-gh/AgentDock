[CmdletBinding()]
param([ValidateSet('Inspect', 'Run')][string]$Mode = 'Inspect',
    [ValidateSet('NativeOnly', 'Layered', 'Offline', 'Editor')][string]$Policy = 'NativeOnly')
$ErrorActionPreference = 'Stop'
$repoRoot = 'C:\Users\wmasa\Documents\Rust\AgentDock_next'
$scriptRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not [string]::Equals($scriptRoot, $repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'This helper must remain in the fixed AgentDock_next verification directory.'
}
$exePath = Join-Path $repoRoot 'app\src-tauri\target\debug\examples\visual_preview_probe.exe'
$probeRoot = Join-Path $repoRoot 'app\src-tauri\target\visual-probe'
if ($Policy -eq 'Editor') {
    $exePath = Join-Path $repoRoot 'app\src-tauri\target\debug\examples\visual_editor_boundary_probe.exe'
    $probeRoot = Join-Path $repoRoot 'app\src-tauri\target\visual-editor-probe'
}

function Assert-NoReparsePath([string]$Path) {
    $checkedPath = [IO.Path]::GetFullPath($Path)
    while ($checkedPath) {
        $checkedItem = $null
        try { $checkedItem = Get-Item -LiteralPath $checkedPath -Force -ErrorAction Stop }
        catch [System.Management.Automation.ItemNotFoundException] { }
        if ($null -ne $checkedItem -and ($checkedItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "ReparsePoint rejected: $checkedPath"
        }
        $checkedParent = [IO.Directory]::GetParent($checkedPath)
        if ($null -eq $checkedParent) { break }
        $checkedPath = $checkedParent.FullName
    }
}
# Preflight guards are not atomic filesystem/concurrency protection. Run launchers sequentially.
Assert-NoReparsePath $PSScriptRoot
Assert-NoReparsePath $exePath
Assert-NoReparsePath $probeRoot

function Get-ProbeProcesses {
    # A failed/inaccessible host query must never become evidence that no process exists.
    $processFilter = if ($Policy -eq 'Editor') { "Name = 'visual_editor_boundary_probe.exe' OR Name = 'visual_preview_probe.exe'" } else { "Name = 'visual_preview_probe.exe'" }
    $candidates = @(Get-CimInstance Win32_Process -Filter $processFilter)
    foreach ($candidate in $candidates) {
        $candidateExe = if ($Policy -eq 'Editor' -and $candidate.Name -eq 'visual_preview_probe.exe') { Join-Path $repoRoot 'app\src-tauri\target\debug\examples\visual_preview_probe.exe' } else { $exePath }
        $exactPathMatch = [string]::Equals($candidate.ExecutablePath, $candidateExe, [StringComparison]::OrdinalIgnoreCase)
        if ($exactPathMatch -or [string]::IsNullOrEmpty($candidate.ExecutablePath)) {
            $started = $null; $responding = $null
            try {
                $native = Get-Process -Id $candidate.ProcessId -ErrorAction Stop
                $started = $native.StartTime.ToUniversalTime().ToString('o')
                $responding = $native.Responding
            } catch { }
            [pscustomobject]@{ pid = $candidate.ProcessId; parentPid = $candidate.ParentProcessId
                path = $candidate.ExecutablePath; exactPathMatch = $exactPathMatch
                startedUtc = $started; responding = $responding }
        }
    }
}
function Get-ProbeInspection {
    Assert-NoReparsePath $exePath
    Assert-NoReparsePath $probeRoot
    $binary = $null
    if (Test-Path -LiteralPath $exePath -PathType Leaf) {
        $item = Get-Item -LiteralPath $exePath
        $binary = [pscustomobject]@{ path = $exePath; bytes = $item.Length
            modifiedUtc = $item.LastWriteTimeUtc.ToString('o')
            sha256 = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash }
    }
    $evidence = @()
    if (Test-Path -LiteralPath $probeRoot -PathType Container) {
        $files = @(foreach ($directory in Get-ChildItem -LiteralPath $probeRoot -Directory) {
            if ($directory.Name -notmatch '^(run|launcher)-[0-9a-fA-F-]+$') { continue }
            Assert-NoReparsePath $directory.FullName
            foreach ($name in @('report.json', 'progress.json', 'progress.jsonl', 'progress.log', 'stdout.log', 'stderr.log', 'launch.json')) {
                $candidatePath = Join-Path $directory.FullName $name
                Assert-NoReparsePath $candidatePath
                if (Test-Path -LiteralPath $candidatePath -PathType Leaf) { Get-Item -LiteralPath $candidatePath }
            }
        })
        $evidence = @($files | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 8 | ForEach-Object {
            Assert-NoReparsePath $_.FullName
            $content = [string](Get-Content -LiteralPath $_.FullName -Tail 80 | Out-String)
            if ($content.Length -gt 16384) { $content = $content.Substring($content.Length - 16384) }
            [pscustomobject]@{ path = $_.FullName; bytes = $_.Length
                modifiedUtc = $_.LastWriteTimeUtc.ToString('o'); tail = $content }
        })
    }
    [pscustomobject]@{ schema = 1; host = $env:COMPUTERNAME; mode = $Mode; policy = $Policy
        inspectedUtc = [DateTime]::UtcNow.ToString('o'); expectedExe = $exePath
        binary = $binary; processes = @(Get-ProbeProcesses); evidenceRoot = $probeRoot; evidence = $evidence }
}

if ($Mode -eq 'Inspect') { Get-ProbeInspection | ConvertTo-Json -Depth 8; return }
if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) { throw "Probe binary missing: $exePath" }
$existing = @(Get-ProbeProcesses)
if ($existing.Count -gt 0) {
    $existing | ConvertTo-Json -Depth 4 | Write-Output
    throw 'Probe already running, or a same-name process path is unavailable. No launch or stop performed.'
}
$launcherPath = Join-Path $probeRoot ('launcher-' + [Guid]::NewGuid().ToString('N'))
$resolvedLauncher = [IO.Path]::GetFullPath($launcherPath)
if (-not $resolvedLauncher.StartsWith($probeRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Launcher output escaped the fixed probe directory.'
}
Assert-NoReparsePath $resolvedLauncher
[IO.Directory]::CreateDirectory($resolvedLauncher) | Out-Null
Assert-NoReparsePath $resolvedLauncher
$stdoutPath = Join-Path $resolvedLauncher 'stdout.log'
$stderrPath = Join-Path $resolvedLauncher 'stderr.log'
Assert-NoReparsePath $stdoutPath
Assert-NoReparsePath $stderrPath
Assert-NoReparsePath $exePath
$startOptions = @{ FilePath = $exePath; WorkingDirectory = $repoRoot; WindowStyle = 'Hidden'
    PassThru = $true; RedirectStandardOutput = $stdoutPath; RedirectStandardError = $stderrPath }
if ($Policy -eq 'Layered') { $startOptions['ArgumentList'] = '--layered-csp' }
if ($Policy -eq 'Offline') { $startOptions['ArgumentList'] = '--offline-preview' }
$probeProcess = Start-Process @startOptions
$launch = [ordered]@{ pid = $probeProcess.Id; executable = $exePath; policy = $Policy
    launchedUtc = [DateTime]::UtcNow.ToString('o'); state = 'launched'; stdout = $stdoutPath; stderr = $stderrPath }
$launchFile = Join-Path $resolvedLauncher 'launch.json'
Assert-NoReparsePath $launchFile
[IO.File]::WriteAllText($launchFile, ($launch | ConvertTo-Json))
$finished = $probeProcess.WaitForExit(25000)
$probeProcess.Refresh()
$launch['state'] = if ($finished) { 'exited' } else { 'still-running-after-25-seconds' }
$launch['exitCode'] = if ($finished) { $probeProcess.ExitCode } else { $null }
$launch['checkedUtc'] = [DateTime]::UtcNow.ToString('o')
Assert-NoReparsePath $launchFile
[IO.File]::WriteAllText($launchFile, ($launch | ConvertTo-Json))
[pscustomobject]@{ launch = $launch; inspection = Get-ProbeInspection } | ConvertTo-Json -Depth 10
# A timeout reports the observed process. Never stop it or another app automatically.
if ($finished) {
    if ($null -eq $launch['exitCode']) { throw 'Probe exited but its native exit code is unavailable.' }
    exit $launch['exitCode']
}
exit 5