# Agentic Desktop Layer installer for Windows. https://w3bs.org/desktop
#
#   irm https://w3bs.org/desktop/install.ps1 | iex
#
# Inspect first if you like:  irm https://w3bs.org/desktop/install.ps1 -OutFile install.ps1; notepad install.ps1; .\install.ps1
# Options (from a saved copy, or with the one-liner as a script block):
#   & ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Version 0.9.0 -NoService
#   & ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Uninstall [-Purge]
#
# What this does, for the current user only (no administrator rights needed):
#   1. Downloads w3bs-desktop-VERSION-x86_64-pc-windows-msvc.zip and SHA256SUMS from the GitHub
#      release desktop-vVERSION and checks the zip against SHA256SUMS.
#   2. Installs it to %LOCALAPPDATA%\Programs\w3bs-desktop and adds its bin folder to your PATH.
#   3. Registers the broker and the notification-area tray to start when you sign in (HKCU Run
#      entries for w3bs-desktopw.exe, the windowless build, so no console window opens), and
#      starts both now.
#   It never enables agent access. The broker always starts Disabled; you enable it yourself
#   from the tray icon.
#
# Options: -Version (default below, or $env:W3BS_DESKTOP_VERSION), -NoService (install the
#          files only, start nothing; also $env:W3BS_DESKTOP_NO_SERVICE=1), -Uninstall (stop it,
#          remove autostart, PATH entry and files; keeps your agents and settings unless -Purge).
# Testing: $env:W3BS_DESKTOP_ARCHIVE = a local zip, with $env:W3BS_DESKTOP_SHA256 = its SHA-256;
#          used instead of downloading, still checked.
#
# Works in Windows PowerShell 5.1 and PowerShell 7. Nothing runs until the last line, so a
# truncated download does nothing.

function Install-W3bsDesktop {
    [CmdletBinding()]
    param(
        [string]$Version = $(if ($env:W3BS_DESKTOP_VERSION) { $env:W3BS_DESKTOP_VERSION } else { '0.9.0' }),
        [switch]$NoService,
        [switch]$Uninstall,
        [switch]$Purge
    )
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # the progress bar makes 5.1 downloads crawl

    function Say([string]$msg) { Write-Host $msg }
    function Fail([string]$msg) { throw "w3bs-desktop installer: $msg" }

    if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set' }
    $root = Join-Path $env:LOCALAPPDATA 'Programs\w3bs-desktop'
    $binDir = Join-Path $root 'bin'
    $cli = Join-Path $binDir 'w3bs-desktop.exe'
    $bg = Join-Path $binDir 'w3bs-desktopw.exe'
    $data = Join-Path $env:LOCALAPPDATA 'w3bs\desktop'
    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    $runBroker = 'W3BS Desktop Broker'
    $runTray = 'W3BS Desktop Tray'

    # Tell Explorer and new terminals that the user environment changed.
    function Send-EnvironmentChange {
        if (-not ('W3bs.NativeEnv' -as [type])) {
            Add-Type -Namespace W3bs -Name NativeEnv -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG
        [void][W3bs.NativeEnv]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
    }

    # Edit the user PATH as stored (REG_EXPAND_SZ, %VARS% unexpanded), not the expanded copy.
    function Set-UserPath([bool]$present) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try {
            $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $parts = @($raw -split ';' | Where-Object { $_ -ne '' })
            $others = @($parts | Where-Object { $_.TrimEnd('\') -ine $binDir.TrimEnd('\') })
            $want = if ($present) { @($others) + $binDir } else { $others }
            if (($want -join ';') -eq ($parts -join ';')) { return $false }
            if ($want.Count -eq 0) { $key.DeleteValue('Path', $false) }
            else { $key.SetValue('Path', ($want -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString) }
        } finally { $key.Close() }
        Send-EnvironmentChange
        return $true
    }

    function Get-OurProcesses {
        @(Get-Process -Name 'w3bs-desktop', 'w3bs-desktopw' -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($binDir + '\', [StringComparison]::OrdinalIgnoreCase) })
    }

    # The broker and tray (the tray's exit, or the broker's, leaves access disabled).
    function Stop-OurProcesses {
        $procs = Get-OurProcesses
        if ($procs.Count -eq 0) { return }
        foreach ($p in $procs) {
            try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch { }
        }
        foreach ($p in $procs) { try { [void]$p.WaitForExit(10000) } catch { } }
        Say "Stopped $($procs.Count) running w3bs-desktop process(es)"
    }

    # Run the CLI, discarding its output; returns the exit code. Windows PowerShell 5.1 turns a
    # native command's stderr into terminating errors under ErrorActionPreference=Stop.
    function Invoke-Cli([string[]]$argv) {
        $eap = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try { & $cli @argv 2>&1 | Out-Null; return $LASTEXITCODE } finally { $ErrorActionPreference = $eap }
    }

    function Remove-Autostart {
        foreach ($name in $runBroker, $runTray) {
            Remove-ItemProperty -Path $runKey -Name $name -ErrorAction SilentlyContinue
        }
    }

    if ($Uninstall) {
        Stop-OurProcesses
        Remove-Autostart
        if (Set-UserPath $false) { Say "Removed $binDir from your PATH" }
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
        Say "Agentic Desktop Layer removed from $root (autostart entries and PATH entry too)."
        if ($Purge) {
            if (Test-Path -LiteralPath $data) { Remove-Item -LiteralPath $data -Recurse -Force }
            $parent = Split-Path -Parent $data
            if ((Test-Path -LiteralPath $parent) -and -not (Get-ChildItem -LiteralPath $parent -Force)) { Remove-Item -LiteralPath $parent -Force }
            Say "Removed your agents, settings and logs ($data)."
        } else {
            Say "Kept your agents, settings and logs in $data (uninstall with -Purge to remove them)."
        }
        return
    }

    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-.+][0-9A-Za-z.]+)?$') { Fail "not a version: $Version" }
    if (-not $NoService -and $env:W3BS_DESKTOP_NO_SERVICE -eq '1') { $NoService = [switch]$true }
    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
    switch ($arch) {
        'AMD64' { }
        'ARM64' { Say 'note: Windows on ARM runs the x86_64 build under emulation' }
        default { Fail "unsupported CPU $arch; x86_64 (and ARM64 under emulation) are supported" }
    }
    $target = 'x86_64-pc-windows-msvc'
    $zipName = "w3bs-desktop-$Version-$target.zip"
    $base = "https://github.com/profullstack/w3bs.org/releases/download/desktop-v$Version"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("w3bs-desktop-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null
    try {
        $zip = Join-Path $tmp $zipName
        if ($env:W3BS_DESKTOP_ARCHIVE) {
            # Testing: a local archive, checked against the checksum given with it.
            $want = $env:W3BS_DESKTOP_SHA256
            if (-not $want) { Fail 'W3BS_DESKTOP_ARCHIVE needs W3BS_DESKTOP_SHA256 (an archive is never installed unchecked)' }
            Say "Installing Agentic Desktop Layer from $($env:W3BS_DESKTOP_ARCHIVE)"
            Copy-Item -LiteralPath $env:W3BS_DESKTOP_ARCHIVE -Destination $zip
        } else {
            # Windows PowerShell 5.1 may still default to TLS 1.0; GitHub needs 1.2.
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            Say "Downloading Agentic Desktop Layer $Version for $target"
            $sums = Join-Path $tmp 'SHA256SUMS'
            try { Invoke-WebRequest -UseBasicParsing -Uri "$base/$zipName" -OutFile $zip } catch { Fail "download failed: $base/$zipName ($($_.Exception.Message))" }
            try { Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums } catch { Fail "download failed: $base/SHA256SUMS ($($_.Exception.Message))" }
            $want = $null
            foreach ($line in Get-Content -LiteralPath $sums) {
                if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(\S+)\s*$' -and $Matches[2] -eq $zipName) { $want = $Matches[1] }
            }
            if (-not $want) { Fail "$zipName is not listed in SHA256SUMS" }
        }
        $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
        if ($got -ine $want.Trim()) { Fail "checksum mismatch for $zipName (expected $want, got $got)" }

        $x = Join-Path $tmp 'x'
        try { Expand-Archive -LiteralPath $zip -DestinationPath $x -Force } catch { Fail "cannot unpack $zipName ($($_.Exception.Message))" }
        foreach ($f in 'bin\w3bs-desktop.exe', 'bin\w3bs-desktopw.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $x $f))) { Fail "$zipName has no $f" }
        }

        # Running copies hold their .exe open; stop them before replacing the files.
        Stop-OurProcesses
        New-Item -ItemType Directory -Path $binDir -Force | Out-Null
        foreach ($f in 'bin\w3bs-desktop.exe', 'bin\w3bs-desktopw.exe', 'LICENSE', 'README.md') {
            $src = Join-Path $x $f
            if (Test-Path -LiteralPath $src) { Copy-Item -LiteralPath $src -Destination (Join-Path $root $f) -Force }
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    $eap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { $ver = (& $cli --version 2>$null) -join ' '; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $eap }
    if ($code -ne 0 -or -not $ver) { Fail "$cli does not run (exit $code)" }
    Say "Installed $cli ($ver)"

    if (Set-UserPath $true) { Say "Added $binDir to your PATH (open a new terminal to use it)" }
    if (-not (($env:Path -split ';') -contains $binDir)) { $env:Path = "$env:Path;$binDir" }

    if (-not $NoService) {
        # Sign-in autostart without console windows: w3bs-desktopw.exe is the GUI-subsystem build.
        New-Item -Path $runKey -Force -ErrorAction SilentlyContinue | Out-Null
        Set-ItemProperty -Path $runKey -Name $runBroker -Value "`"$bg`" daemon --adapter auto --mcp-http 127.0.0.1:7801"
        Set-ItemProperty -Path $runKey -Name $runTray -Value "`"$bg`" tray --wait-for-broker 60"
        Say "Broker and tray registered to start when you sign in (HKCU Run: '$runBroker', '$runTray')"
        $interactive = [Environment]::UserInteractive -and (Get-Process -Id $PID).SessionId -ne 0
        if ($interactive) {
            Start-Process -FilePath $bg -ArgumentList 'daemon', '--adapter', 'auto', '--mcp-http', '127.0.0.1:7801' -WorkingDirectory $root
            Start-Process -FilePath $bg -ArgumentList 'tray', '--wait-for-broker', '60' -WorkingDirectory $root
            $up = $false
            for ($i = 0; $i -lt 60 -and -not $up; $i++) {
                if ((Invoke-Cli @('status')) -eq 0) { $up = $true } else { Start-Sleep -Milliseconds 250 }
            }
            if ($up) { Say 'Broker running (agent access: Disabled); the tray icon is in the notification area' }
            else { Say "note: the broker did not answer yet; see $data\state\logs\daemon.log" }
        } else {
            Say 'Not an interactive session; the broker and tray start when you next sign in'
        }
    }

    Say ''
    Say 'Next:'
    Say '  1. Register an agent:   w3bs-desktop agents add claude-code --preset broad'
    Say '  2. Connect it:          w3bs-desktop setup claude-code --agent claude-code --http   (prints the claude mcp add line)'
    Say '     OpenCode instead:    w3bs-desktop agents add opencode --preset broad; w3bs-desktop setup opencode --agent opencode --http   (prints the opencode.json mcp block)'
    Say '  3. Enable access from the tray icon when you want the agent to work; disable it the same way.'
    Say '     Emergency stop: Win+Alt+Esc. Quitting the tray disables access.'
    Say "Logs: $data\state\logs   Uninstall: & ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Uninstall"
    Say 'Docs: https://w3bs.org/desktop'
}

Install-W3bsDesktop @args
