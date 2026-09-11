# start-bot.ps1 - Antigravity Discord Remote 포그라운드 실행
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptDir

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  Antigravity Discord Remote (Rust)" -ForegroundColor Green
Write-Host "=========================================" -ForegroundColor Cyan

# .env 파일 검사
$EnvFile = Join-Path $ScriptDir ".env"
if (-not (Test-Path $EnvFile)) {
    Write-Host "[오류] .env 파일이 없습니다. .env.example을 참고하여 생성해 주세요." -ForegroundColor Red
    exit 1
}

$EnvContent = Get-Content $EnvFile -Raw
if ($EnvContent -match "여기에_디스코드_봇_토큰을_붙여넣으세요" -or $EnvContent -match "your_bot_token_here") {
    Write-Host "[알림] .env 파일에 아직 디스코드 봇 토큰이 입력되지 않았습니다!" -ForegroundColor Yellow
    Write-Host "C:\utube\agy-discord-remote\.env 파일을 열어서 DISCORD_BOT_TOKEN에 토큰을 입력해 주세요." -ForegroundColor Yellow
    exit 1
}

$ExePath = Join-Path $ScriptDir "target\release\agy-discord-remote.exe"
if (-not (Test-Path $ExePath)) {
    Write-Host "[빌드] 릴리즈 바이너리를 빌드합니다..." -ForegroundColor Yellow
    $env:PATH = "C:\Users\ihavg\.cargo\bin;" + $env:PATH
    cargo build --release
}

Write-Host "[시작] 봇 런타임을 시작합니다... (종료: Ctrl + C)" -ForegroundColor Green
& $ExePath

