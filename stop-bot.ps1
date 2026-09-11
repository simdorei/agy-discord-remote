# stop-bot.ps1 - Antigravity Discord Remote 프로세스 종료
$ErrorActionPreference = "SilentlyContinue"

Write-Host "Antigravity Discord Remote 및 감시 프로세스를 종료합니다..." -ForegroundColor Yellow

# 1. 메인 봇 프로세스 종료
Get-Process -Name "agy-discord-remote" | Stop-Process -Force
Write-Host "agy-discord-remote 프로세스 정지 완료." -ForegroundColor Green

# 2. 왓치독 파워쉘 프로세스 종료
$watchdogs = Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*watchdog.ps1*" }
foreach ($wd in $watchdogs) {
    Stop-Process -Id $wd.ProcessId -Force
    Write-Host "왓치독 프로세스 (PID: $($wd.ProcessId)) 정지 완료." -ForegroundColor Green
}

Write-Host "모든 프로세스가 안전하게 종료되었습니다." -ForegroundColor Cyan

