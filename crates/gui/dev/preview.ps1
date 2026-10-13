<#
DEV ONLY. Builds a browser preview of the MailBackup UI with a fake backend (mock-tauri.js) and,
optionally, takes headless Microsoft Edge screenshots of scripted states.

  powershell -ExecutionPolicy Bypass -File crates\gui\dev\preview.ps1                 # build preview only
  powershell -ExecutionPolicy Bypass -File crates\gui\dev\preview.ps1 -Screenshots    # + PNGs in docs\screenshots

The preview is written to target\ui-preview\index.html (open it in a browser; append
#wizard1, #progress, ... to see a state). Nothing here talks to a mail server or the credential store.
#>
param(
    [switch]$Screenshots,
    [string]$OutDir = "",
    [int]$Width = 1180,
    [int]$Height = 760,
    [switch]$Dark
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$ui = Join-Path $repo "crates\gui\ui"
$dst = Join-Path $repo "target\ui-preview"
New-Item -ItemType Directory -Force $dst | Out-Null
Copy-Item -Force (Join-Path $ui "*") $dst
Copy-Item -Force (Join-Path $PSScriptRoot "mock-tauri.js") $dst
$html = [IO.File]::ReadAllText((Join-Path $ui "index.html"))
$html = $html.Replace('<script src="app.js"></script>', '<script src="mock-tauri.js"></script><script src="app.js"></script>')
[IO.File]::WriteAllText((Join-Path $dst "index.html"), $html, (New-Object Text.UTF8Encoding($false)))
Write-Output "preview: $dst\index.html"
if (-not $Screenshots) { return }

if ($OutDir -eq "") { $OutDir = Join-Path $repo "docs\screenshots" }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$edge = @(
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw "Microsoft Edge not found" }
$profileDir = Join-Path $dst "edge-profile"
$scenarios = "empty", "accounts", "wizard1", "wizard-error", "wizard2", "wizard3", "wizard4", "progress", "migrate", "partial", "error", "ok", "export", "export-imap", "remove"
$url = "file:///" + ($dst -replace "\\", "/") + "/index.html"
foreach ($s in $scenarios) {
    $suffix = if ($Dark) { "-dark" } else { "" }
    $png = Join-Path $OutDir "ui-$s$suffix.png"
    $edgeArgs = @("--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
        "--user-data-dir=$profileDir", "--window-size=$Width,$Height", "--virtual-time-budget=5000",
        "--screenshot=$png")
    if ($Dark) { $edgeArgs += "--force-dark-mode"; $edgeArgs += "--blink-settings=preferredColorScheme=0" }
    else { $edgeArgs += "--blink-settings=preferredColorScheme=1" }
    $edgeArgs += "$url#$s"
    # msedge.exe is a GUI program: wait for it explicitly.
    Start-Process -FilePath $edge -ArgumentList $edgeArgs -Wait -WindowStyle Hidden
    Write-Output "screenshot: $png"
}
