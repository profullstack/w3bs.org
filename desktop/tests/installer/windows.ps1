# Installer end to end on Windows (GitHub's windows-latest runner, an interactive desktop
# session): runs packaging/install.ps1 under Windows PowerShell 5.1 against a locally built
# release zip, the way a person would (`irm | iex`, a saved copy with options, the script-block
# uninstall), and checks what they get.
#
#   pwsh -File tests/installer/windows.ps1 -Archive ZIP -Version X.Y.Z [-Work DIR]   (from desktop/)
#
# Prints ok/FAIL per check; exits 1 if anything failed. Logs land in -Work.
param(
    [Parameter(Mandatory)] [string]$Archive,
    [Parameter(Mandatory)] [string]$Version,
    [string]$Work = (Join-Path ($env:RUNNER_TEMP ?? $env:TEMP) 'installer-win')
)
$ErrorActionPreference = 'Continue'
$Archive = (Resolve-Path $Archive).Path
$Ps1 = (Resolve-Path 'packaging/install.ps1').Path
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Work | Out-Null

$root = Join-Path $env:LOCALAPPDATA 'Programs\w3bs-desktop'
$binDir = Join-Path $root 'bin'
$cli = Join-Path $binDir 'w3bs-desktop.exe'
$bg = Join-Path $binDir 'w3bs-desktopw.exe'
$data = Join-Path $env:LOCALAPPDATA 'w3bs\desktop'
$logs = Join-Path $data 'state\logs'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$sha = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash

$script:fails = 0
$script:passes = 0
function ok($msg) { $script:passes++; Write-Host "ok   $msg" }
function bad($msg) { $script:fails++; Write-Host "FAIL $msg" }
function check($cond, $msg, $detail = '') { if ($cond) { ok $msg } else { bad "$msg $detail" } }
function until($block, [int]$ms = 20000) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt $ms) { if (& $block) { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}
# Run the installer in Windows PowerShell 5.1 (powershell.exe) with a -Command string.
function Installer([string]$command, [string]$log, [string]$sum = $sha) {
    $env:W3BS_DESKTOP_ARCHIVE = $Archive
    $env:W3BS_DESKTOP_SHA256 = $sum
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -Command $command *> (Join-Path $Work $log)
    $code = $LASTEXITCODE
    Remove-Item Env:W3BS_DESKTOP_ARCHIVE, Env:W3BS_DESKTOP_SHA256 -ErrorAction SilentlyContinue
    Get-Content (Join-Path $Work $log) | ForEach-Object { Write-Host "   | $_" }
    return $code
}
function procs([string]$kind) {
    @(Get-CimInstance Win32_Process -Filter "Name = 'w3bs-desktopw.exe' OR Name = 'w3bs-desktop.exe'" |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($binDir, [StringComparison]::OrdinalIgnoreCase) -and $_.CommandLine -match " $kind( |$)" })
}
function conhost-children([int[]]$ids) {
    @(Get-CimInstance Win32_Process -Filter "Name = 'conhost.exe'" | Where-Object { $ids -contains [int]$_.ParentProcessId })
}
function status {
    $out = & $cli --json status 2>$null
    try { ($out -join "`n") | ConvertFrom-Json } catch { $null }
}
# PE optional header Subsystem: 2 = Windows GUI, 3 = console.
function subsystem([string]$path) {
    $b = [IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    [BitConverter]::ToUInt16($b, $pe + 0x5C)
}
function user-path {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try { [string]$k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } finally { $k.Close() }
}
function save-logs([string]$tag) {
    if (Test-Path $logs) { Copy-Item -Recurse -Force $logs (Join-Path $Work "logs-$tag") }
}

Write-Host "== session: $env:USERNAME session=$((Get-Process -Id $PID).SessionId) interactive=$([Environment]::UserInteractive) PS=$($PSVersionTable.PSVersion)"
Write-Host "== Windows PowerShell: $(& powershell.exe -NoProfile -Command '$PSVersionTable.PSVersion.ToString()')"

# ------------------------------------------------------------------ refusal
$code = Installer "Get-Content -Raw -LiteralPath '$Ps1' | Invoke-Expression" 'mismatch.txt' ('0' * 64)
check ($code -ne 0) "checksum mismatch is refused (exit $code)"
check ((Get-Content -Raw (Join-Path $Work 'mismatch.txt')) -match 'checksum mismatch') "mismatch message names the checksum"
check (-not (Test-Path $root)) "nothing installed after a refused archive"

# ------------------------------------------------------------------ install (one-liner form)
$code = Installer "Get-Content -Raw -LiteralPath '$Ps1' | Invoke-Expression" 'install.txt'
check ($code -eq 0) "install via | Invoke-Expression under Windows PowerShell 5.1 exits 0 (exit $code)"
check ((Test-Path $cli) -and (Test-Path $bg)) "w3bs-desktop.exe and w3bs-desktopw.exe installed in $binDir"
check ((Test-Path (Join-Path $root 'LICENSE')) -and (Test-Path (Join-Path $root 'README.md'))) "LICENSE and README.md installed"
$ver = (& $cli --version) -join ' '
check ($ver -match " $([regex]::Escape($Version))$") "w3bs-desktop --version is $Version" $ver
check ((subsystem $cli) -eq 3) "w3bs-desktop.exe is a console-subsystem binary" (subsystem $cli)
check ((subsystem $bg) -eq 2) "w3bs-desktopw.exe is a GUI-subsystem binary (no console)" (subsystem $bg)
$run = Get-ItemProperty -Path $runKey
check ($run.'W3BS Desktop Broker' -eq "`"$bg`" daemon --adapter auto") "HKCU Run 'W3BS Desktop Broker' starts w3bs-desktopw daemon at sign-in" $run.'W3BS Desktop Broker'
check ($run.'W3BS Desktop Tray' -eq "`"$bg`" tray --wait-for-broker 60") "HKCU Run 'W3BS Desktop Tray' starts w3bs-desktopw tray at sign-in" $run.'W3BS Desktop Tray'
check ((user-path) -split ';' -contains $binDir) "bin folder added to the user PATH (HKCU\Environment)" (user-path)
$installText = Get-Content -Raw (Join-Path $Work 'install.txt')
check ($installText -match 'agents add' -and $installText -match 'setup claude-code' -and $installText -match 'Win\+Alt\+Esc') "installer printed next steps (agents add, setup, Win+Alt+Esc)"
check (-not ((Get-Content -Raw $Ps1) -match 'enable\.(request|confirm)')) "installer never enables access"

check (until { (procs 'daemon').Count -eq 1 }) "one broker process running"
$broker = (procs 'daemon') | Select-Object -First 1
check ($broker -and $broker.Name -eq 'w3bs-desktopw.exe') "broker runs from the GUI-subsystem w3bs-desktopw.exe" "$($broker.Name) $($broker.CommandLine)"
check (until { (procs 'tray').Count -eq 1 }) "one tray process running"
$tray = (procs 'tray') | Select-Object -First 1
check ($tray -and $tray.Name -eq 'w3bs-desktopw.exe') "tray runs from the GUI-subsystem w3bs-desktopw.exe" "$($tray.Name) $($tray.CommandLine)"
check (until { $s = status; $s -and $s.state -eq 'disabled' }) "w3bs-desktop status answers Disabled through the installed broker"
$s = status
check ($s.adapter -eq 'windows-native') "broker picked the windows-native adapter (--adapter auto)" ($s | ConvertTo-Json -Compress)
check (until { (Get-Content -Raw (Join-Path $logs 'daemon.log') -ErrorAction SilentlyContinue) -match 'broker ready' }) "broker output goes to state\logs\daemon.log"
check (until { (Get-Content -Raw (Join-Path $logs 'tray.log') -ErrorAction SilentlyContinue) -match 'notification icon added' } 60000) "tray added its notification-area icon (tray.log)" (Get-Content -Raw (Join-Path $logs 'tray.log') -ErrorAction SilentlyContinue)
check (until { (Get-Content -Raw (Join-Path $logs 'tray.log') -ErrorAction SilentlyContinue) -match 'state Disabled' }) "tray subscribed to the broker and shows Disabled"
Start-Sleep -Seconds 2
$kids = conhost-children @([int]$broker.ProcessId, [int]$tray.ProcessId)
check ($kids.Count -eq 0) "no conhost.exe (console window host) under the broker or the tray" ($kids | ForEach-Object { "conhost pid $($_.ProcessId) parent $($_.ParentProcessId)" })

# Positive control for the conhost check: the console build started the same way gets one.
$ctl = Join-Path $Work 'control'
$env:W3BS_DESKTOP_RUNTIME_DIR = Join-Path $ctl 'run'
$env:W3BS_DESKTOP_CONFIG_DIR = Join-Path $ctl 'config'
$env:W3BS_DESKTOP_STATE_DIR = Join-Path $ctl 'state'
$cp = Start-Process -FilePath $cli -ArgumentList 'daemon', '--adapter', 'mock' -WindowStyle Hidden -PassThru
Remove-Item Env:W3BS_DESKTOP_RUNTIME_DIR, Env:W3BS_DESKTOP_CONFIG_DIR, Env:W3BS_DESKTOP_STATE_DIR
check (until { (conhost-children @([int]$cp.Id)).Count -ge 1 } 10000) "control: the console build started detached does get a conhost.exe child (the check can see one)"
Stop-Process -Id $cp.Id -Force -ErrorAction SilentlyContinue
[void]$cp.WaitForExit(10000)
save-logs 'install'

# ------------------------------------------------------------------ re-install (saved copy, options)
$oldIds = @([int]$broker.ProcessId, [int]$tray.ProcessId)
$code = Installer "& '$Ps1' -Version $Version" 'reinstall.txt'
check ($code -eq 0) "re-install from a saved copy with -Version exits 0 (exit $code)"
check (until { $b = procs 'daemon'; $b.Count -eq 1 -and $oldIds -notcontains [int]$b[0].ProcessId }) "re-install replaced the running broker (one new process)"
check (until { $t = procs 'tray'; $t.Count -eq 1 -and $oldIds -notcontains [int]$t[0].ProcessId }) "re-install replaced the running tray (one new process)"
check (until { $s = status; $s -and $s.state -eq 'disabled' }) "status answers Disabled after re-install"
check (@((user-path) -split ';' | Where-Object { $_ -eq $binDir }).Count -eq 1) "PATH entry not duplicated by re-install"
$add = & $cli --json agents add installer-test --preset observe 2>&1
check ($LASTEXITCODE -eq 0 -and (Test-Path (Join-Path $data 'config\agents.json'))) "an agent registered (config to keep across uninstall)" ($add -join ' ')
save-logs 'reinstall'

# ------------------------------------------------------------------ uninstall (script-block form)
$code = Installer "& ([scriptblock]::Create((Get-Content -Raw -LiteralPath '$Ps1'))) -Uninstall" 'uninstall.txt'
check ($code -eq 0) "uninstall via the script-block form with -Uninstall exits 0 (exit $code)"
check (until { (procs 'daemon').Count -eq 0 -and (procs 'tray').Count -eq 0 } 10000) "broker and tray stopped"
$run = Get-ItemProperty -Path $runKey
check (-not $run.'W3BS Desktop Broker' -and -not $run.'W3BS Desktop Tray') "HKCU Run entries removed"
check (-not ((user-path) -split ';' -contains $binDir)) "bin folder removed from the user PATH" (user-path)
check (-not (Test-Path $root)) "install folder removed"
check (Test-Path (Join-Path $data 'config\agents.json')) "agents and settings kept without -Purge"
$code = Installer "& ([scriptblock]::Create((Get-Content -Raw -LiteralPath '$Ps1'))) -Uninstall -Purge" 'purge.txt'
check ($code -eq 0 -and -not (Test-Path $data)) "-Uninstall -Purge removes agents, settings and logs (exit $code)"

Write-Host "== $script:passes passed, $script:fails failed"
if ($script:fails -gt 0) { exit 1 }
