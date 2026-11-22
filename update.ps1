# Update Create Panel on Windows from a new source zip: unpack, build, install, open.
# Usage: double-click update.bat, or: powershell -ExecutionPolicy Bypass -File update.ps1 [zip] [-Release] [-Installer]
# -Installer also makes a proper Create-Panel-Setup.exe (Inno Setup, installed automatically with winget if missing) and runs it.
# Needs Rust (https://rustup.rs) and the "Desktop development with C++" build tools once.
# Source and build cache live in %USERPROFILE%\.create-panel-build, so later runs only rebuild what changed.
param([string]$Zip = "", [switch]$Release, [switch]$Installer)
$ErrorActionPreference = "Stop"
function Pause-Exit($code) { Read-Host "Press Enter to close"; exit $code }
try {
  $base = Join-Path $env:USERPROFILE ".create-panel-build"; $src = Join-Path $base "src"
  $env:CARGO_TARGET_DIR = Join-Path $base "target"
  $mode = if ($Release) { "release" } else { "debug" }; $flag = if ($Release) { @("--release") } else { @() }
  if (-not $Zip) { $z = Get-ChildItem (Join-Path $env:USERPROFILE "Downloads\create-panel-source*.zip") -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1; if ($z) { $Zip = $z.FullName } }
  if (-not $Zip -or -not (Test-Path $Zip)) { Write-Host "I can't find a new zip. Put create-panel-source.zip in Downloads, or pass its path."; Pause-Exit 1 }
  $cargo = Get-Command cargo -ErrorAction SilentlyContinue
  if (-not $cargo -and (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) { $env:PATH += ";$env:USERPROFILE\.cargo\bin"; $cargo = Get-Command cargo -ErrorAction SilentlyContinue }
  if (-not $cargo) { Write-Host "Rust (cargo) isn't installed. Get it from https://rustup.rs, then run this again."; Pause-Exit 1 }
  Write-Host "Using: $Zip"
  Write-Host "This closes Create Panel while it updates."
  $a = Read-Host "Continue? (y/N)"; if ($a -ne "y" -and $a -ne "Y") { exit 0 }
  New-Item -ItemType Directory -Force $src | Out-Null
  if (Test-Path $Zip -PathType Container) { Get-ChildItem $Zip -Force | Where-Object { $_.Name -ne "target" -and $_.Name -ne ".git" } | Copy-Item -Destination $src -Recurse -Force } else { Expand-Archive -Path $Zip -DestinationPath $src -Force }
  if (-not (Test-Path (Join-Path $src "Cargo.toml"))) { $c = Get-ChildItem $src -Directory | Where-Object { Test-Path (Join-Path $_.FullName "Cargo.toml") } | Select-Object -First 1; if ($c) { $src = $c.FullName } }
  if (-not (Test-Path (Join-Path $src "Cargo.toml"))) { Write-Host "That zip doesn't look right."; Pause-Exit 1 }
  Write-Host "Building ($mode)..."
  Push-Location $src
  cargo build @flag; if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Host "The build failed. Copy the error above and send it to Claude. Your installed app is unchanged."; Pause-Exit 1 }
  Push-Location gui
  cargo build @flag; if ($LASTEXITCODE -ne 0) { Pop-Location; Pop-Location; Write-Host "The app build failed. Copy the error above and send it to Claude. Your installed app is unchanged."; Pause-Exit 1 }
  Pop-Location; Pop-Location
  Get-Process create-panel-app, create-panel -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep -Seconds 1
  if ($Installer) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { Write-Host "Installing Inno Setup (the installer maker)..."; winget install --id JRSoftware.InnoSetup -e --silent --accept-package-agreements --accept-source-agreements | Out-Null
      $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1 }
    if (-not $iscc) { Write-Host "Couldn't get Inno Setup. Install it from https://jrsoftware.org/isdl.php and run again."; Pause-Exit 1 }
    $bin = Join-Path $env:CARGO_TARGET_DIR $mode
    & $iscc "/DBinDir=$bin" "/DGuiBinDir=$bin" (Join-Path $src "installer\create-panel.iss"); if ($LASTEXITCODE -ne 0) { Write-Host "Making the installer failed."; Pause-Exit 1 }
    $setup = Join-Path $src "dist\Create-Panel-Setup.exe"; Copy-Item $setup (Join-Path ([Environment]::GetFolderPath("Desktop")) "Create-Panel-Setup.exe") -Force
    Write-Host "Made Create-Panel-Setup.exe (also on your Desktop). Opening it..."; Start-Process $setup; exit 0
  }
  $dest = Join-Path $env:LOCALAPPDATA "Programs\Create Panel"; New-Item -ItemType Directory -Force $dest | Out-Null
  Copy-Item (Join-Path $env:CARGO_TARGET_DIR "$mode\create-panel.exe") $dest -Force
  Copy-Item (Join-Path $env:CARGO_TARGET_DIR "$mode\create-panel-app.exe") $dest -Force
  Copy-Item (Join-Path $src "gui\assets\icon.ico") $dest -Force
  $lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Create Panel.lnk"
  $sh = New-Object -ComObject WScript.Shell; $s = $sh.CreateShortcut($lnk)
  $s.TargetPath = Join-Path $dest "create-panel-app.exe"; $s.WorkingDirectory = $dest; $s.IconLocation = Join-Path $dest "icon.ico"; $s.Save()
  Write-Host "Updated. Opening it..."
  Start-Process (Join-Path $dest "create-panel-app.exe")
} catch { Write-Host "Something went wrong: $_"; Pause-Exit 1 }
