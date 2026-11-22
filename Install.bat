@echo off
rem START HERE (Windows): double-click to build and install Create Panel from this folder.
rem Also makes Create-Panel-Setup.exe on your Desktop (a normal installer you can keep).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0update.ps1" -Zip "%~dp0." -Release -Installer
