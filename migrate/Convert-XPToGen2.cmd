@echo off
rem Converts an XP disk (Hyper-V Generation 1, Integration Services 6.3) for Generation 2.
rem Drag the .vhd/.vhdx/.avhdx onto this file, or double-click it and enter the path.
rem Any further arguments go to Convert-XPToGen2.ps1, e.g.:
rem   Convert-XPToGen2.cmd D:\VMs\xp.vhdx -VMName "XP Gen2" -SwitchName "Default Switch"
rem PowerShell asks for elevation and keeps its window open until Enter is pressed.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Convert-XPToGen2.ps1" -Pause %*
