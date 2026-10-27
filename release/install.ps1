# Verdict installer for Windows. Run it from the downloaded release folder:
#   powershell -ExecutionPolicy Bypass -File install.ps1
# It verifies the binary against the checksum beside it, installs it under %LOCALAPPDATA%\Verdict,
# registers the MCP server for the hosts it finds, and opens the Verdict page. No terminal step is
# needed after it finishes.
param(
  [string]$Version = "1.0.0",
  [string]$From = "$PSScriptRoot"
)
$ErrorActionPreference = "Stop"
$arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { throw "Verdict needs 64-bit Windows" }
$bin = Join-Path $From "vdt-$Version-$arch-pc-windows-msvc.exe"
if (-not (Test-Path $bin)) { throw "cannot find $bin beside the installer" }
$sumFile = "$bin.sha256"
if (Test-Path $sumFile) {
  $want = (Get-Content $sumFile -Raw).Split()[0].ToLower()
  $got = (Get-FileHash $bin -Algorithm SHA256).Hash.ToLower()
  if ($want -ne $got) { throw "checksum mismatch: the download is damaged or was changed" }
} else {
  Write-Warning "no checksum beside $bin; installation continues, but the download is unverified"
}
$dir = Join-Path $env:LOCALAPPDATA "Verdict\bin"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$target = Join-Path $dir "vdt.exe"
Copy-Item $bin $target -Force
# Make vdt findable for hooks in new shells, without touching any other PATH entry.
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if (-not $userPath.Split(';').Contains($dir)) {
  [Environment]::SetEnvironmentVariable("PATH", "$dir;$userPath", "User")
}
$env:PATH = "$dir;$env:PATH"

# A desktop shortcut that starts the page: the installer's front door.
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath("Desktop")) "Verdict.lnk"))
$shortcut.TargetPath = $target
$shortcut.Arguments = "serve"
$shortcut.WorkingDirectory = $env:USERPROFILE
$shortcut.Description = "Verdict: start the local page"
$shortcut.Save()

# Register the MCP server where the hosts are installed.
$mcp = '{"command":"' + ($target -replace '\\','/') + '","args":["mcp"]}'
foreach ($pair in @(@("$env:USERPROFILE\.claude", "claude"), @("$env:USERPROFILE\.cursor", "cursor"))) {
  if (Test-Path $pair[0]) {
    Write-Host "MCP for $($pair[1]): $mcp"
  }
}

Write-Host "Installed $target"
Write-Host "Starting the Verdict page..."
Start-Process $target -ArgumentList "serve" -WorkingDirectory $env:USERPROFILE
Start-Sleep -Seconds 1
Start-Process "http://127.0.0.1:4717/"
Write-Host "Verdict is running at http://127.0.0.1:4717/ (close that window to stop it)."
