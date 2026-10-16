# W3BS Training installer for Windows. https://w3bs.org/training
#
#   irm https://w3bs.org/training/install.ps1 | iex
#
# Inspect first if you like:  irm https://w3bs.org/training/install.ps1 -OutFile install.ps1; notepad install.ps1; .\install.ps1
#
# What this does, for the current user only (no administrator rights needed):
#   1. Downloads w3bs-training-VERSION-x86_64-pc-windows-msvc.zip and SHA256SUMS from the GitHub
#      release training-vVERSION and checks the zip against SHA256SUMS.
#   2. Installs the single self-contained w3bs-training.exe to
#      %LOCALAPPDATA%\Programs\w3bs-training\bin and adds that folder to your PATH.
#   Recordings are encrypted with a key protected by Windows (DPAPI, your user account only).
#   It starts nothing. The Agentic Desktop Layer's recorder hands recordings to it.
#
# Options: -Version (default below, or $env:W3BS_TRAINING_VERSION), -Uninstall (removes the
#          program and its PATH entry; your data in %LOCALAPPDATA%\w3bs\training stays).
# Testing: $env:W3BS_TRAINING_ARCHIVE = a local zip, with $env:W3BS_TRAINING_SHA256 = its SHA-256.
#
# Works in Windows PowerShell 5.1 and PowerShell 7. Nothing runs until the last line.

function Install-W3bsTraining {
    [CmdletBinding()]
    param(
        [string]$Version = $(if ($env:W3BS_TRAINING_VERSION) { $env:W3BS_TRAINING_VERSION } else { '0.2.3' }),
        [switch]$Uninstall
    )
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    function Say([string]$msg) { Write-Host $msg }
    function Fail([string]$msg) { throw "w3bs-training installer: $msg" }

    if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set' }
    $root = Join-Path $env:LOCALAPPDATA 'Programs\w3bs-training'
    $binDir = Join-Path $root 'bin'
    $exe = Join-Path $binDir 'w3bs-training.exe'

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
        return $true
    }

    if ($Uninstall) {
        if (Set-UserPath $false) { Say "Removed $binDir from your PATH" }
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
        Say "W3BS Training removed. Your data in $env:LOCALAPPDATA\w3bs\training is kept; delete that folder to remove it too."
        return
    }

    $zipName = "w3bs-training-$Version-x86_64-pc-windows-msvc.zip"
    $base = "https://github.com/profullstack/w3bs.org/releases/download/training-v$Version"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("w3bs-training-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp $zipName
        if ($env:W3BS_TRAINING_ARCHIVE) {
            if (-not $env:W3BS_TRAINING_SHA256) { Fail 'W3BS_TRAINING_ARCHIVE needs W3BS_TRAINING_SHA256 (an archive is never installed unchecked)' }
            Copy-Item -LiteralPath $env:W3BS_TRAINING_ARCHIVE -Destination $zip
            $want = $env:W3BS_TRAINING_SHA256
        } else {
            Say "Downloading W3BS Training $Version"
            $sums = Join-Path $tmp 'SHA256SUMS'
            try { Invoke-WebRequest -UseBasicParsing -Uri "$base/$zipName" -OutFile $zip } catch { Fail "download failed: $base/$zipName ($($_.Exception.Message))" }
            try { Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums } catch { Fail "download failed: $base/SHA256SUMS ($($_.Exception.Message))" }
            $line = Get-Content -LiteralPath $sums | Where-Object { $_ -match "\s$([regex]::Escape($zipName))$" } | Select-Object -First 1
            if (-not $line) { Fail "$zipName is not listed in SHA256SUMS" }
            $want = ($line -split '\s+')[0]
        }
        $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
        if ($got -ine $want) { Fail "checksum mismatch for $zipName (expected $want, got $got)" }
        $x = Join-Path $tmp 'x'
        try { Expand-Archive -LiteralPath $zip -DestinationPath $x -Force } catch { Fail "cannot unpack $zipName ($($_.Exception.Message))" }
        New-Item -ItemType Directory -Path $binDir -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $x 'bin\w3bs-training.exe') -Destination $exe -Force
        Copy-Item -LiteralPath (Join-Path $x 'LICENSE'), (Join-Path $x 'README.md') -Destination $root -Force
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    & $exe version | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "$exe does not run on this machine" }
    if (Set-UserPath $true) { Say "Added $binDir to your PATH (open a new terminal; restart the desktop layer's broker so it finds it)" }
    Say "Installed $exe $Version"
    Say 'Next: w3bs-training open   (the review app in your browser). Record from the tray: Start training...'
}

Install-W3bsTraining @args
