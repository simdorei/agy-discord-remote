# start-detached.ps1 - 봇을 WMI를 통해 부모 세션과 완전히 분리하여 독립 백그라운드로 실행한다.
$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$ExePath = Join-Path $ScriptDir "target\release\agy-discord-remote.exe"

if (-not (Test-Path $ExePath)) {
    Write-Host "[오류] 실행 파일이 없습니다: $ExePath" -ForegroundColor Red
    exit 1
}

# 기존 프로세스 정리
$running = Get-Process -Name "agy-discord-remote" -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "[정리] 기존 프로세스 종료 중 (PID: $($running.Id -join ', '))" -ForegroundColor Yellow
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
}

$LogOut = Join-Path $ScriptDir "bot-run.stdout.log"
$LogErr = Join-Path $ScriptDir "bot-run.stderr.log"
$CommandLine = "cmd.exe /c cd /d `"$ScriptDir`" && `"$ExePath`" >> `"$LogOut`" 2>> `"$LogErr`""

$res = Invoke-WmiMethod -Class Win32_Process -Name Create -ArgumentList $CommandLine
if ($res.ReturnValue -ne 0) {
    Write-Host "[오류] WMI 프로세스 생성 실패: ReturnValue $($res.ReturnValue)" -ForegroundColor Red
    exit 1
}

Start-Sleep -Seconds 2
$bot = Get-Process -Name "agy-discord-remote" -ErrorAction SilentlyContinue

if ($bot) {
    Write-Host "[시작] 봇 독립 실행 중 (PID: $($bot.Id -join ', '))" -ForegroundColor Green
    Write-Host "       로그: $LogOut / $LogErr" -ForegroundColor DarkGray
} else {
    Write-Host "[확인 필요] 프로세스가 보이지 않습니다. 로그를 확인하세요: $LogErr" -ForegroundColor Yellow
}
