# Verdict installer for Windows.
#
#   Double-click the file, or: powershell -ExecutionPolicy Bypass -File install.ps1
#
# It shows a small window: choose a project folder, register the Verdict MCP server with the hosts
# you use, install vdt, and open the page. For automation and the release tests it also runs with
# -Silent. After it finishes there is no terminal step: every decision is a button on the page.
param(
  [string]$Version = "1.0.0",
  [string]$From = "$PSScriptRoot",
  [string]$InstallDir = "$env:LOCALAPPDATA\Verdict\bin",
  [string]$Project = (Get-Location).Path,
  [switch]$Silent,
  [switch]$NoShortcut,
  [switch]$NoLaunch,
  [switch]$NoMcp
)
$ErrorActionPreference = "Stop"

function Find-Binary {
  $arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { throw "Verdict needs 64-bit Windows" }
  $names = @(
    "vdt-$Version-$arch-pc-windows-msvc.exe",
    "vdt-$Version-$arch-pc-windows-msvc.zip",
    "vdt.exe"
  )
  foreach ($n in $names) {
    $p = Join-Path $From $n
    if (Test-Path $p) { return $p }
  }
  throw "cannot find a vdt binary beside the installer ($From)"
}

function Verify-Checksum([string]$path) {
  $sumFile = "$path.sha256"
  if (-not (Test-Path $sumFile)) {
    Write-Warning "no checksum beside $path; the download is unverified"
    return
  }
  $want = (Get-Content $sumFile -Raw).Split()[0].ToLower()
  $got = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
  if ($want -ne $got) { throw "checksum mismatch: the download is damaged or was changed" }
}

function Install-Verdict {
  param([string]$ProjectDir, [bool]$RegisterHosts, [bool]$MakeShortcut, [bool]$OpenPage)

  $bin = Find-Binary
  if ($bin.ToLower().EndsWith(".zip")) {
    $unzip = Join-Path $env:TEMP ("vdt-install-" + [guid]::NewGuid().ToString("N"))
    Expand-Archive -LiteralPath $bin -DestinationPath $unzip -Force
    $bin = Get-ChildItem $unzip -Recurse -Filter "vdt.exe" | Select-Object -First 1 -ExpandProperty FullName
  }
  Verify-Checksum $bin
  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  $target = Join-Path $InstallDir "vdt.exe"
  Copy-Item $bin $target -Force

  $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
  if (-not ($userPath -split ';').Contains($InstallDir)) {
    [Environment]::SetEnvironmentVariable("PATH", "$InstallDir;$userPath", "User")
  }
  $env:PATH = "$InstallDir;$env:PATH"

  if ($RegisterHosts -and -not $NoMcp) {
    foreach ($hostName in @("claude", "cursor")) {
      & $target mcp-install $hostName --project $ProjectDir --i-am-the-human --vdt-path $target
      if ($LASTEXITCODE -ne 0) { throw "registering the MCP server with $hostName failed (exit $LASTEXITCODE)" }
    }
  }

  if ($MakeShortcut -and -not $NoShortcut) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath("Desktop")) "Verdict.lnk"))
    $shortcut.TargetPath = $target
    $shortcut.Arguments = "serve"
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.Description = "Verdict: start the local page"
    $shortcut.Save()
  }

  if ($OpenPage -and -not $NoLaunch) {
    Start-Process $target -ArgumentList "serve" -WorkingDirectory $ProjectDir
    Start-Sleep -Seconds 1
    Start-Process "http://127.0.0.1:4717/"
  }
  return $target
}

function Run-Gui {
  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing

  $form = New-Object System.Windows.Forms.Form
  $form.Text = "Verdict installer"
  $form.Size = New-Object System.Drawing.Size(640, 440)
  $form.StartPosition = "CenterScreen"
  $form.BackColor = [System.Drawing.Color]::White

  $title = New-Object System.Windows.Forms.Label
  $title.Text = "Verdict 1.0.0"
  $title.Font = New-Object System.Drawing.Font("Segoe UI", 20, [System.Drawing.FontStyle]::Bold)
  $title.AutoSize = $true
  $title.Location = New-Object System.Drawing.Point(20, 16)
  $form.Controls.Add($title)

  $blurb = New-Object System.Windows.Forms.Label
  $blurb.Text = "Checks the work of AI coding agents. The agent writes claims; verdicts come from checks, another vendor or you."
  $blurb.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $blurb.Size = New-Object System.Drawing.Size(560, 50)
  $blurb.Location = New-Object System.Drawing.Point(22, 62)
  $form.Controls.Add($blurb)

  $label = New-Object System.Windows.Forms.Label
  $label.Text = "Project folder:"
  $label.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $label.Location = New-Object System.Drawing.Point(22, 124)
  $label.AutoSize = $true
  $form.Controls.Add($label)

  $box = New-Object System.Windows.Forms.TextBox
  $box.Text = $Project
  $box.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $box.Location = New-Object System.Drawing.Point(22, 148)
  $box.Size = New-Object System.Drawing.Size(460, 28)
  $form.Controls.Add($box)

  $register = New-Object System.Windows.Forms.CheckBox
  $register.Text = "Register the Verdict MCP server with Claude Code and Cursor (this project)"
  $register.Checked = $true
  $register.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $register.Location = New-Object System.Drawing.Point(22, 190)
  $register.Size = New-Object System.Drawing.Size(560, 26)
  $form.Controls.Add($register)

  $shortcut = New-Object System.Windows.Forms.CheckBox
  $shortcut.Text = "Put a Verdict shortcut on the desktop"
  $shortcut.Checked = $true
  $shortcut.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $shortcut.Location = New-Object System.Drawing.Point(22, 218)
  $shortcut.Size = New-Object System.Drawing.Size(560, 26)
  $form.Controls.Add($shortcut)

  $open = New-Object System.Windows.Forms.CheckBox
  $open.Text = "Open the Verdict page when installation finishes"
  $open.Checked = $true
  $open.Font = New-Object System.Drawing.Font("Segoe UI", 11)
  $open.Location = New-Object System.Drawing.Point(22, 246)
  $open.Size = New-Object System.Drawing.Size(560, 26)
  $form.Controls.Add($open)

  $status = New-Object System.Windows.Forms.Label
  $status.Text = "Installs to $InstallDir. No terminal step is needed after this."
  $status.Font = New-Object System.Drawing.Font("Segoe UI", 10)
  $status.ForeColor = [System.Drawing.Color]::DimGray
  $status.Location = New-Object System.Drawing.Point(22, 282)
  $status.Size = New-Object System.Drawing.Size(330, 44)
  $form.Controls.Add($status)

  $install = New-Object System.Windows.Forms.Button
  $install.Text = "Install Verdict"
  $install.Font = New-Object System.Drawing.Font("Segoe UI", 12, [System.Drawing.FontStyle]::Bold)
  $install.Size = New-Object System.Drawing.Size(200, 46)
  $install.Location = New-Object System.Drawing.Point(390, 296)
  $install.Add_Click({
    try {
      $target = Install-Verdict -ProjectDir $box.Text -RegisterHosts $register.Checked -MakeShortcut $shortcut.Checked -OpenPage $open.Checked
      $status.Text = "Installed $target"
      [System.Windows.Forms.MessageBox]::Show("Verdict is installed.`n`n$target`n`nThe page opens when you press OK.", "Verdict", "OK", "Information") | Out-Null
      if ($open.Checked) { Start-Process "http://127.0.0.1:4717/" }
      $form.Close()
    } catch {
      $status.Text = "Install failed: $($_.Exception.Message)"
      [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, "Verdict installer", "OK", "Error") | Out-Null
    }
  })
  $form.Controls.Add($install)

  [void]$form.ShowDialog()
}

if ($Silent) {
  $target = Install-Verdict -ProjectDir $Project -RegisterHosts (-not $NoMcp) -MakeShortcut (-not $NoShortcut) -OpenPage (-not $NoLaunch)
  Write-Host "Installed $target"
  exit 0
}
Run-Gui
