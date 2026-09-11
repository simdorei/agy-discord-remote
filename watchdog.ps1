# watchdog.ps1 - Antigravity Discord Remote 무중단 감시 프로세스
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptDir

$ExePath = Join-Path $ScriptDir "target\release\agy-discord-remote.exe"
$LogFile = Join-Path $ScriptDir "bot.log"

Write-Output "[$(Get-Date)] Antigravity Discord Remote 왓치독 시작" | Tee-Object -FilePath $LogFile -Append

while ($true) {
    if (Test-Path $ExePath) {
        Write-Output "[$(Get-Date)] 봇 프로세스 시작" | Tee-Object -FilePath $LogFile -Append
        try {
            $BotRunTag = Get-Date -Format "yyyyMMdd-HHmmss-fff"
            $BotStdoutLog = Join-Path $ScriptDir "bot-$BotRunTag.stdout.log"
            $BotStderrLog = Join-Path $ScriptDir "bot-$BotRunTag.stderr.log"
            $process = Start-Process -FilePath $ExePath -WorkingDirectory $ScriptDir -WindowStyle Hidden -RedirectStandardOutput $BotStdoutLog -RedirectStandardError $BotStderrLog -PassThru -Wait
            Write-Output "[$(Get-Date)] 봇 프로세스 종료됨 (ExitCode: $($process.ExitCode)). 3초 후 재시작합니다." | Tee-Object -FilePath $LogFile -Append
        } catch {
            Write-Output "[$(Get-Date)] 실행 예외: $_" | Tee-Object -FilePath $LogFile -Append
        }
    } else {
        Write-Output "[$(Get-Date)] $ExePath 파일을 찾을 수 없습니다." | Tee-Object -FilePath $LogFile -Append
    }
    Start-Sleep -Seconds 3
}
