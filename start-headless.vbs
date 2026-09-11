' start-headless.vbs - Antigravity Discord Remote 무창(Hidden) 백그라운드 실행기
Set WshShell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
ScriptDir = fso.GetParentFolderName(WScript.ScriptFullName)

Command = "powershell.exe -ExecutionPolicy Bypass -NoProfile -WindowStyle Hidden -File """ & ScriptDir & "\watchdog.ps1"""
WshShell.Run Command, 0, False
Set WshShell = Nothing

