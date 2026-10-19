' start-headless.vbs - launch the bot hidden via start-detached.ps1.
' start-detached.ps1 is WMI-detached (survives console/session cleanup) and
' refuses to start a second instance, so it is safe to run at every logon.
' (Previous version ran watchdog.ps1, a resident loop WITHOUT a duplicate
'  check, which could spawn a second bot instance and cause gateway conflicts.)
Set WshShell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
ScriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
Command = "powershell.exe -ExecutionPolicy Bypass -NoProfile -WindowStyle Hidden -File """ & ScriptDir & "\start-detached.ps1"""
WshShell.Run Command, 0, False
Set WshShell = Nothing
