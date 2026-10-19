Start-Sleep -Seconds 3
Get-Process -Name "agy-discord-remote" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
Set-Location "C:\utube\agy-discord-remote"
& "C:\utube\agy-discord-remote\start-detached.ps1"
