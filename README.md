# Antigravity Discord Remote (Rust)

로컬 PC에서 실행 중인 **Google Antigravity (`agy.exe`)** 에이전트를 모바일 및 외부 디스코드에서 제어하는 고성능 Rust 런타임입니다.

---

## 📌 주요 특징
* **모바일 최적화**: 스마트폰 디스코드 앱으로 어디서든 작업 요청 및 결과 확인
* **스레드 자동 생성**: `!task <내용>` 입력 시 전용 디스코드 스레드가 생성되어 대화 분리
* **문맥 기억 (Continuous Context)**: 생성된 스레드 안에서 대화하면 이전 작업 내용을 기억하고 이어서 코딩 수행
* **24시간 무중단 백그라운드 구동**: 윈도우 창 없이 백그라운드에서 실행되며 크래시 시 왓치독이 자동 재시작
* **보안 화이트리스트**: 내 디스코드 유저 ID만 허용하여 타인의 무단 사용 원천 차단

---

## 🚀 빠른 시작 가이드 (2분 완료)

### 1단계: 디스코드 봇 생성 및 토큰 발급
1. [Discord Developer Portal](https://discord.com/developers/applications)에 접속합니다.
2. 우측 상단 **New Application** 클릭 $\rightarrow$ 이름 입력 (예: `Antigravity Remote`)
3. 좌측 메뉴 **Bot** 클릭 $\rightarrow$ **Reset Token** 클릭하여 토큰 복사
4. 같은 **Bot** 페이지 아래쪽 **Privileged Gateway Intents** 섹션에서:
   * ✅ **MESSAGE CONTENT INTENT** 스위치를 켭니다. (필수)
   * `Save Changes` 클릭
5. 좌측 메뉴 **OAuth2** $\rightarrow$ **URL Generator** 클릭:
   * `SCOPES`: `bot`, `applications.commands` 체크
   * `BOT PERMISSIONS`: `Send Messages`, `Create Public Threads`, `Send Messages in Threads`, `Embed Links`, `Read Message History` 체크
   * 하단 생성된 URL을 복사하여 브라우저에 붙여넣고 **본인의 디스코드 서버에 봇을 초대**합니다.

### 2단계: 로컬 `.env` 설정
`C:\utube\agy-discord-remote\.env` 파일을 메모장이나 VS Code로 열고 복사한 토큰을 붙여넣습니다:
```env
DISCORD_BOT_TOKEN=여기에_발급받은_봇_토큰_붙여넣기
ALLOWED_USER_ID=내_디스코드_유저_아이디 (선택사항, 권장)
```
*(내 디스코드 유저 ID 확인법: 디스코드 설정 -> 고급 -> '개발자 모드' 켬 -> 내 프로필 우클릭 -> '사용자 ID 복사')*

### 3단계: 실행
* **디버그/직접 실행 (콘솔 창 확인용)**:
  `start-bot.ps1` 우클릭 $\rightarrow$ `PowerShell에서 실행`
* **완전 무창 백그라운드 실행 (상시 가동용)**:
  `start-headless.vbs` 더블 클릭! (콘솔 창 없이 백그라운드에서 조용히 돌아갑니다)
* **종료할 때**:
  `stop-bot.ps1` 우클릭 $\rightarrow$ `PowerShell에서 실행`

---

## 💬 디스코드 명령어
| 명령어 | 설명 |
| :--- | :--- |
| `!usage` (또는 `!quota`, `!사용량`) | 모델별 주간 / 5시간 쿼터 잔여량(%) 및 리셋 시각 즉시 확인 |
| `!task <작업 내용>` | 새 안티그래비티 작업을 시작합니다. (전용 스레드가 자동 생성됩니다) |
| `!new <작업 내용>` | `!task`와 동일 |
| `스레드 안에서 대화` | 해당 작업의 이전 맥락을 기억하며 대화를 이어나갑니다. |
| `!status` | 현재 연결된 안티그래비티 바이너리, 작업 공간 경로 확인 |
| `!help` | 도움말 안내 임베드 출력 |
