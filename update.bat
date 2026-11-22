@echo off
rem Double-click to update Create Panel from the newest create-panel-source*.zip in Downloads.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0update.ps1" %*
