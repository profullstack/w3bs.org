# A deterministic test window for the Windows end-to-end run: a form whose multiline TextBox
# fills it. Every 100 ms the form writes its state (text, HWND, focus, bounds) as JSON to -Out.
param([string]$Title, [string]$Out, [int]$X, [int]$Y, [int]$W = 480, [int]$H = 320)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$f = New-Object System.Windows.Forms.Form
$f.Text = $Title
$f.StartPosition = 'Manual'
$f.Location = New-Object System.Drawing.Point($X, $Y)
$f.Size = New-Object System.Drawing.Size($W, $H)
$tb = New-Object System.Windows.Forms.TextBox
$tb.Multiline = $true
$tb.Dock = 'Fill'
$tb.Name = 'Input'
$tb.AccessibleName = "$Title input"
$tb.BackColor = [System.Drawing.Color]::White
$f.Controls.Add($tb)
$utf8 = New-Object System.Text.UTF8Encoding($false)
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 100
$t.Add_Tick({
    $state = [ordered]@{
        text = $tb.Text
        handle = ('0x{0:x}' -f $f.Handle.ToInt64())
        focused = $f.ContainsFocus
        left = $f.Left; top = $f.Top; width = $f.Width; height = $f.Height
    } | ConvertTo-Json -Compress
    $tmp = "$Out.tmp"
    [System.IO.File]::WriteAllText($tmp, $state, $utf8)
    Move-Item -Force $tmp $Out
})
$t.Start()
$f.Add_Shown({ $f.Activate() })
[System.Windows.Forms.Application]::Run($f)
