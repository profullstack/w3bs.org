# End-to-end run of the Windows native adapter on a real interactive desktop (GitHub's
# windows-latest runner, or any Windows session). Drives the release binary exactly as a person
# and an agent would: daemon --adapter native, `control` as the native control, CLI input against
# two WinForms test windows, then the notification-area tray.
#
#   pwsh -File tests/windows/e2e-native.ps1 [-Bin target\release\w3bs-desktop.exe] [-Work DIR]
#
# Prints one line per check (ok/FAIL) and exits 1 if any check failed. Logs land in -Work.
param(
    [string]$Bin = "target\release\w3bs-desktop.exe",
    [string]$Work = (Join-Path ($env:RUNNER_TEMP ?? $env:TEMP) "adl-e2e")
)
$ErrorActionPreference = 'Continue'
$Bin = (Resolve-Path $Bin).Path
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Work | Out-Null
$env:W3BS_DESKTOP_CONFIG_DIR = Join-Path $Work 'config'
$env:W3BS_DESKTOP_STATE_DIR = Join-Path $Work 'state'
$env:W3BS_DESKTOP_RUNTIME_DIR = Join-Path $Work 'run'
Remove-Item Env:W3BS_DESKTOP_TOKEN -ErrorAction SilentlyContinue

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class N {
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern bool GetPhysicalCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    public static int[] Cursor() { POINT p; GetPhysicalCursorPos(out p); return new int[] { p.X, p.Y }; }
    public static void Chord(byte[] keys) {
        foreach (var k in keys) keybd_event(k, 0, (k == 0x5B ? 1u : 0u), UIntPtr.Zero);
        for (int i = keys.Length - 1; i >= 0; i--) keybd_event(keys[i], 0, 2u | (keys[i] == 0x5B ? 1u : 0u), UIntPtr.Zero);
    }
}
"@
[void][N]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
Add-Type -AssemblyName System.Drawing

$script:fails = 0
$script:passes = 0
function ok($msg) { $script:passes++; Write-Host "ok   $msg" }
function bad($msg) { $script:fails++; Write-Host "FAIL $msg" }
function check($cond, $msg, $detail = '') { if ($cond) { ok $msg } else { bad "$msg $detail" } }

# Run the CLI with --json; returns @{code; v; raw}.
function W {
    $out = & $Bin --json @args 2>> (Join-Path $Work 'cli.err')
    $code = $LASTEXITCODE
    $raw = ($out -join "`n")
    $v = $null
    try { $v = $raw | ConvertFrom-Json -Depth 64 } catch { }
    return [pscustomobject]@{ code = $code; v = $v; raw = $raw }
}
function until($block, [int]$ms = 10000) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt $ms) { if (& $block) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
function state { (W status).v.state }
function form($path) { try { Get-Content -Raw -Encoding utf8 $path | ConvertFrom-Json } catch { $null } }
function find-window($list, $title) { $list | Where-Object { $_.title -eq $title } | Select-Object -First 1 }

Write-Host "== session: $env:USERNAME session=$((Get-Process -Id $PID).SessionId) console=$([N]::WTSGetActiveConsoleSessionId()) SESSIONNAME=$env:SESSIONNAME"
Write-Host "== screen (physical): $([N]::GetSystemMetrics(0))x$([N]::GetSystemMetrics(1)), virtual $([N]::GetSystemMetrics(76)),$([N]::GetSystemMetrics(77)) $([N]::GetSystemMetrics(78))x$([N]::GetSystemMetrics(79))"
$explorer = Get-Process explorer -ErrorAction SilentlyContinue
Write-Host "== explorer running: $([bool]$explorer)"

# ---------------------------------------------------------------- daemon
$daemon = Start-Process -FilePath $Bin -ArgumentList 'daemon', '--adapter', 'native' -RedirectStandardError (Join-Path $Work 'daemon.err') -RedirectStandardOutput (Join-Path $Work 'daemon.out') -NoNewWindow -PassThru
check (until { (W status).code -eq 0 }) "daemon --adapter native answers on its named pipe"
$st = W status
check ($st.v.state -eq 'disabled' -and $st.v.adapter -eq 'windows-native') "fresh start is Disabled on windows-native" $st.raw
$doc = W doctor
Write-Host ($doc.v.checks | ConvertTo-Json -Compress -Depth 5)
$hk = $doc.v.checks | Where-Object { $_.check -eq 'emergency_hotkey' }
check ($hk.ok) "emergency hotkey registered by the adapter: $($hk.detail)"
$caps = W capabilities
$ptr = $caps.v | Where-Object { $_.capability -eq 'pointer' }
check ($ptr.availability -eq 'available') "pointer capability available ($($ptr.reason))"

$add = W agents add tester --preset broad
check ($add.code -eq 0) "agents add tester --preset broad" $add.raw
$r = W windows list --agent tester
check ($r.code -eq 4 -and $r.v.error.code -eq 'disabled') "agent refused while Disabled" $r.raw

# ---------------------------------------------------------------- native control: `control`
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = 'cmd.exe'
$psi.Arguments = "/c `"`"$Bin`" control > `"$Work\control.log`" 2>&1`""
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.CreateNoWindow = $true
$ctl = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Milliseconds 800
$ctl.StandardInput.WriteLine('enable'); $ctl.StandardInput.Flush()
check (until { (state) -eq 'enabled' }) "control (stdin pipe) enables via enable.request + enable.confirm"

$lay = W layout --agent tester
$disp = $lay.v.data.displays | Where-Object { $_.primary } | Select-Object -First 1
Write-Host "== layout: $($lay.v.data.displays | ConvertTo-Json -Compress -Depth 5)"
check ($disp -and $disp.logical.width -eq [N]::GetSystemMetrics(0) -and $disp.logical.height -eq [N]::GetSystemMetrics(1)) "primary display $($disp.id) is $($disp.logical.width)x$($disp.logical.height) physical px (matches GetSystemMetrics)"

# ---------------------------------------------------------------- test windows
$fa = Join-Path $Work 'a.json'; $fb = Join-Path $Work 'b.json'
$formScript = Join-Path $Here 'testform.ps1'
$pa = Start-Process powershell.exe -ArgumentList '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass', '-File', "`"$formScript`"", '-Title', '"W3BS Test A"', '-Out', "`"$fa`"", '-X', 100, '-Y', 100 -RedirectStandardOutput (Join-Path $Work 'forma.out') -RedirectStandardError (Join-Path $Work 'forma.err') -NoNewWindow -PassThru
Start-Sleep -Milliseconds 1500
$pb = Start-Process powershell.exe -ArgumentList '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass', '-File', "`"$formScript`"", '-Title', '"W3BS Test B"', '-Out', "`"$fb`"", '-X', 300, '-Y', 200 -RedirectStandardOutput (Join-Path $Work 'formb.out') -RedirectStandardError (Join-Path $Work 'formb.err') -NoNewWindow -PassThru
check (until { (Test-Path $fa) -and (Test-Path $fb) } 30000) "two WinForms test windows are up" "A alive=$(-not $pa.HasExited) B alive=$(-not $pb.HasExited) $(Get-Content -Raw (Join-Path $Work 'forma.err') -ErrorAction SilentlyContinue)"
Start-Sleep -Milliseconds 500

$wl = W windows list --agent tester
$wa = find-window $wl.v.data 'W3BS Test A'; $wb = find-window $wl.v.data 'W3BS Test B'
$sa = form $fa; $sb = form $fb
Write-Host "== windows: $($wl.v.data | Select-Object id, app_id, title, focused, @{n='b';e={"$($_.bounds.x),$($_.bounds.y) $($_.bounds.width)x$($_.bounds.height)"}} | ConvertTo-Json -Compress)"
check ($wa -and $wb) "windows list includes both forms"
check ($wa.id -eq $sa.handle -and $wb.id -eq $sb.handle) "window ids are the forms' HWNDs ($($wa.id), $($wb.id))"
function sane($w, $s) {
    $b = $w.bounds
    # DWM frame bounds exclude the invisible resize borders, so they sit inside the form's rect.
    return $b -and $b.x -ge $s.left -and $b.y -ge $s.top -and ($b.x + $b.width) -le ($s.left + $s.width) -and ($b.y + $b.height) -le ($s.top + $s.height) -and $b.width -ge ($s.width - 24) -and $b.height -ge ($s.height - 24)
}
check ((sane $wa $sa) -and (sane $wb $sb)) "window rects match the forms (DWM frame inside the form rect)" "A=$($wa.bounds | ConvertTo-Json -Compress) vs $($sa | ConvertTo-Json -Compress); B=$($wb.bounds | ConvertTo-Json -Compress) vs $($sb | ConvertTo-Json -Compress)"
check ($wa.app_id -eq 'powershell.exe') "app id is the process image name ($($wa.app_id))"
check ((@($wl.v.data | Where-Object { $_.focused })).Count -le 1) "at most one window is focused"

# Point inside B but outside A, and inside A but outside B (A: 100,100 480x320; B: 300,200).
$r = W mouse click --agent tester --window $wb.id --x 400 --y 250
check ($r.code -eq 0) "mouse click on B (window-relative) completes" $r.raw
check (until { (form $fb).focused }) "click focused form B"
$wl = W windows list --agent tester
$focused = @($wl.v.data | Where-Object { $_.focused })
check ($focused.Count -eq 1 -and $focused[0].id -eq $wb.id) "windows list: exactly one focused, and it is B"

# ---------------------------------------------------------------- keyboard
$text = "héllo € 😀 ok"
$tf = Join-Path $Work 'text.txt'
[System.IO.File]::WriteAllText($tf, $text, (New-Object System.Text.UTF8Encoding($false)))
$r = W keyboard type --agent tester --window $wb.id --from-file $tf
check ($r.code -eq 0) "keyboard type --window B completes" $r.raw
check (until { (form $fb).text -eq $text }) "B's TextBox holds exactly the typed text incl. é, € and an emoji (surrogate pair)" "got: $((form $fb).text)"
check ((form $fa).text -eq '') "nothing leaked into A"
$r = W keyboard type --agent tester --window $wa.id --from-file $tf
check ($r.code -ne 0 -and $r.v.error.code -eq 'target_stale') "typing into unfocused A is refused target_stale" $r.raw
check ((form $fa).text -eq '') "A is still empty after the refused type"

# ---------------------------------------------------------------- pointer
$sw = [N]::GetSystemMetrics(0); $sh = [N]::GetSystemMetrics(1)
$allok = $true; $detail = ''
foreach ($p in @(@(500, 333), @(7, 9), @(($sw - 1), ($sh - 1)), @([int]($sw / 2), [int]($sh / 3)))) {
    $r = W mouse move --agent tester --x $p[0] --y $p[1]
    $c = [N]::Cursor()
    if ($r.code -ne 0 -or $c[0] -ne $p[0] -or $c[1] -ne $p[1]) { $allok = $false; $detail += " want $($p -join ',') got $($c -join ',') rc=$($r.code) $($r.raw);" }
}
check $allok "mouse move lands on the exact physical pixel (GetPhysicalCursorPos), corners included" $detail
# A point of A that B covers is refused (occlusion via WindowFromPoint).
$r = W mouse click --agent tester --window $wa.id --x 300 --y 200
check ($r.code -ne 0 -and $r.v.error.code -eq 'target_stale' -and $r.raw -match 'covered') "click on a part of A covered by B is refused target_stale (covered)" $r.raw
$r = W mouse click --agent tester --window $wa.id --x 50 --y 60
check ($r.code -eq 0) "mouse click on A's visible part completes" $r.raw
check (until { (form $fa).focused -and -not (form $fb).focused }) "the click moved focus to form A"
$r = W mouse scroll --agent tester --window $wa.id --x 60 --y 80 --steps 2
check ($r.code -eq 0) "mouse scroll over A completes" $r.raw

# ---------------------------------------------------------------- capture
$shot = Join-Path $Work 'shot.png'
$r = W screen capture --agent tester --display $disp.id --output $shot
check ($r.code -eq 0 -and (Test-Path $shot)) "screen capture --display $($disp.id) writes a file" $r.raw
if (Test-Path $shot) {
    $bytes = [System.IO.File]::ReadAllBytes($shot)
    $pw = [int]$bytes[16] * 16777216 + [int]$bytes[17] * 65536 + [int]$bytes[18] * 256 + [int]$bytes[19]
    $ph = [int]$bytes[20] * 16777216 + [int]$bytes[21] * 65536 + [int]$bytes[22] * 256 + [int]$bytes[23]
    check ([Text.Encoding]::ASCII.GetString($bytes, 1, 3) -eq 'PNG' -and $pw -eq $sw -and $ph -eq $sh) "PNG is ${pw}x${ph}, the monitor's pixel size (${sw}x${sh})"
    $bmp = [System.Drawing.Bitmap]::FromFile($shot)
    $cx = [int]($wa.bounds.x + 60); $cy = [int]($wa.bounds.y + 120)
    $px = $bmp.GetPixel($cx - $disp.logical.x, $cy - $disp.logical.y)
    $bmp.Dispose()
    check ($px.R -gt 230 -and $px.G -gt 230 -and $px.B -gt 230) "captured pixels show A's white TextBox at ($cx,$cy): rgb($($px.R),$($px.G),$($px.B))"
}

# ---------------------------------------------------------------- accessibility
$r = W inspect --agent tester --window $wb.id
function walk($n) { $n; foreach ($c in $n.children) { walk $c } }
$nodes = @(walk $r.v.data)
$edit = $nodes | Where-Object { $_.role -eq 'edit' } | Select-Object -First 1
check ($r.code -eq 0 -and $edit -and $edit.value -eq $text) "inspect B: UI Automation 'edit' node carries the typed value ($($nodes.Count) nodes)" "edit=$($edit | ConvertTo-Json -Compress -Depth 3)"
check ($edit.actions -contains 'set_value') "edit node lists set_value"

# ---------------------------------------------------------------- disable during a long type
$r = W mouse click --agent tester --window $wb.id --x 400 --y 250
check (until { (form $fb).focused }) "B focused again for the long type"
$before = (form $fb).text.Length
$long = Join-Path $Work 'long.txt'
[System.IO.File]::WriteAllText($long, ('x' * 3000))
$lt = Start-Process -FilePath $Bin -ArgumentList '--json', 'keyboard', 'type', '--agent', 'tester', '--window', $wb.id, '--from-file', $long -RedirectStandardOutput (Join-Path $Work 'long.out') -RedirectStandardError (Join-Path $Work 'long.err') -NoNewWindow -PassThru
check (until { (form $fb).text.Length -gt $before + 20 }) "long type is landing in B"
$d = W disable
check ($d.code -eq 0 -and $d.v.tasks_cancelled -ge 1) "disable mid-type cancels the task (gate closed in $($d.v.gate_closed_ms) ms)" $d.raw
$lt.WaitForExit(15000) | Out-Null
$lo = Get-Content -Raw (Join-Path $Work 'long.out')
check ($lo -match 'cancel') "the type command reports cancelled" $lo
Start-Sleep -Milliseconds 400
$n1 = (form $fb).text.Length - $before
Start-Sleep -Milliseconds 600
$n2 = (form $fb).text.Length - $before
check ($n1 -gt 0 -and $n1 -lt 3000 -and $n1 -eq $n2) "typing stopped part-way ($n1 of 3000 characters) and stays stopped"
$r = W mouse move --agent tester --x 10 --y 10
check ($r.code -eq 4 -and $r.v.error.code -eq 'disabled') "input after disable exits 4 (disabled)" $r.raw

# ---------------------------------------------------------------- emergency hotkey
$ctl.StandardInput.WriteLine('enable'); $ctl.StandardInput.Flush()
check (until { (state) -eq 'enabled' }) "re-enabled from the control"
$desc = $hk.detail
[N]::Chord([byte[]](0x5B, 0x12, 0x1B))   # Win + Alt + Esc
$fired = until { (state) -eq 'disabled' } 5000
if (-not $fired -and $desc -match 'F12') { [N]::Chord([byte[]](0x11, 0x12, 0x10, 0x7B)); $fired = until { (state) -eq 'disabled' } 5000 }
$audit = W audit export
check ($fired -and $audit.raw -match 'emergency stop') "emergency chord ($desc, injected) disables, audited as emergency stop"

# ---------------------------------------------------------------- takeover by other input
$ctl.StandardInput.WriteLine('enable'); $ctl.StandardInput.Flush()
check (until { (state) -eq 'enabled' }) "re-enabled for the takeover check"
[N]::Chord([byte[]](0x1B))   # close anything the injected Win chord may have opened
Start-Sleep -Milliseconds 600   # let the broker see this outside input while idle (not a takeover)
$r = W mouse click --agent tester --window $wb.id --x 400 --y 250
check (until { (form $fb).focused }) "B focused for the takeover check" $r.raw
$before = (form $fb).text.Length
$lt = Start-Process -FilePath $Bin -ArgumentList '--json', 'keyboard', 'type', '--agent', 'tester', '--window', $wb.id, '--from-file', $long -RedirectStandardOutput (Join-Path $Work 'take.out') -RedirectStandardError (Join-Path $Work 'take.err') -NoNewWindow -PassThru
until { (form $fb).text.Length -gt $before + 20 } | Out-Null
[N]::mouse_event(1, 3, 0, 0, [UIntPtr]::Zero)   # a relative move from another process, as a person would
check (until { (state) -eq 'disabled' } 5000) "input from outside the broker during agent typing = human takeover -> disabled"
$lt.WaitForExit(15000) | Out-Null
$audit = W audit export
check ($audit.raw -match 'human takeover') "audited as human takeover"

# ---------------------------------------------------------------- tray
$ctl.StandardInput.WriteLine('quit'); $ctl.StandardInput.Flush()
$ctl.WaitForExit(5000) | Out-Null
$trayErr = Join-Path $Work 'tray.err'
$tray = Start-Process -FilePath $Bin -ArgumentList 'tray' -RedirectStandardError $trayErr -RedirectStandardOutput (Join-Path $Work 'tray.out') -NoNewWindow -PassThru
check (until { (Get-Content -Raw $trayErr -ErrorAction SilentlyContinue) -match 'notification icon added' } 35000) "tray: Shell_NotifyIcon(NIM_ADD) succeeded" (Get-Content -Raw $trayErr -ErrorAction SilentlyContinue)
check (until { (Get-Content -Raw $trayErr) -match 'state Disabled' }) "tray: subscribed as native control, shows Disabled"
$th = [N]::FindWindow('W3bsDesktopTray', [NullString]::Value)
check ($th -ne [IntPtr]::Zero) "tray window found (class W3bsDesktopTray)"
$WM_COMMAND = 0x0111; $WM_TRAY = 0x8001; $WM_CANCELMODE = 0x001F
[void][N]::PostMessage($th, $WM_TRAY, [IntPtr]::Zero, [IntPtr]0x007B)   # WM_CONTEXTMENU from the icon
check (until { (Get-Content -Raw $trayErr) -match 'menu: Status: Disabled.*Enable agent access.*Stop all agent activity.*Quit and disable' }) "tray menu opens with status in words, Enable, Stop all, Quit and disable"
[void][N]::PostMessage($th, $WM_CANCELMODE, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 300
[N]::Chord([byte[]](0x1B))
Start-Sleep -Milliseconds 300
[void][N]::PostMessage($th, $WM_COMMAND, [IntPtr]1, [IntPtr]::Zero)   # "Enable agent access"
check (until { (state) -eq 'enabled' }) "tray Enable (menu command) enables: enable.request + enable.confirm"
check ((W windows list --agent tester).code -eq 0) "agent works with the tray as the only native control"
[void][N]::PostMessage($th, $WM_TRAY, [IntPtr]::Zero, [IntPtr]0x0400)   # NIN_SELECT (left click)
check (until { (state) -eq 'disabled' }) "tray left click while enabled disables"
[void][N]::PostMessage($th, $WM_TRAY, [IntPtr]::Zero, [IntPtr]0x0400)
Start-Sleep -Milliseconds 800
check ((state) -eq 'disabled') "tray left click while disabled does not enable"

if ($explorer) {
    [void][N]::PostMessage($th, $WM_COMMAND, [IntPtr]1, [IntPtr]::Zero)
    check (until { (state) -eq 'enabled' }) "tray enabled before restarting Explorer"
    Stop-Process -Name explorer -Force
    $lost = until { (state) -eq 'disabled' } 15000
    check $lost "Explorer gone: the tray disables access (the switch is not visible)" (Get-Content -Raw $trayErr)
    if (-not (until { Get-Process explorer -ErrorAction SilentlyContinue } 8000)) { Start-Process explorer.exe }
    check (until { (Get-Content -Raw $trayErr) -match 'icon re-added: True' } 30000) "Explorer restarted: TaskbarCreated -> icon re-added" (Get-Content -Raw $trayErr)
} else {
    Write-Host "skip Explorer restart: explorer.exe is not running in this session"
}

[void][N]::PostMessage($th, $WM_COMMAND, [IntPtr]1, [IntPtr]::Zero)
until { (state) -eq 'enabled' } | Out-Null
[void][N]::PostMessage($th, $WM_COMMAND, [IntPtr]5, [IntPtr]::Zero)   # "Quit and disable"
check ($tray.WaitForExit(10000) -and (state) -eq 'disabled') "tray Quit and disable: tray exited, broker disabled"

# ---------------------------------------------------------------- cleanup
foreach ($p in @($pa, $pb, $daemon)) { try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch { } }
Write-Host ""
Write-Host "windows native e2e: $script:passes passed, $script:fails failed"
if ($script:fails -gt 0) {
    foreach ($f in 'daemon.err', 'control.log', 'tray.err', 'cli.err', 'long.err', 'long.out', 'forma.out', 'forma.err', 'formb.err') {
        $p = Join-Path $Work $f
        if (Test-Path $p) { Write-Host "---- $f"; Get-Content $p | Select-Object -Last 60 }
    }
    exit 1
}
exit 0
