use crate::agy::{AgyProgress, AgyRunner};
use crate::config::Config;
use crate::store::{SessionInfo, SessionStore};
use serenity::all::*;
use serenity::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, mpsc};
use tokio::task::AbortHandle;
use tracing::{error, info, warn};

pub struct Handler {
    pub config: Config,
    pub agy: AgyRunner,
    pub store: SessionStore,
    pub current_workspace: Arc<RwLock<PathBuf>>,
    pub current_model: Arc<RwLock<String>>,
    pub active_tasks: Arc<RwLock<HashMap<u64, AbortHandle>>>,
}

impl Handler {
    pub fn new(config: Config, agy: AgyRunner, store: SessionStore) -> Self {
        let current_workspace = Arc::new(RwLock::new(config.default_workspace.clone()));
        let current_model = Arc::new(RwLock::new(config.default_model.clone()));
        let active_tasks = Arc::new(RwLock::new(HashMap::new()));
        Self {
            config,
            agy,
            store,
            current_workspace,
            current_model,
            active_tasks,
        }
    }

    async fn abort_channel_task(&self, channel_id: u64) -> bool {
        let mut map = self.active_tasks.write().await;
        if let Some(handle) = map.remove(&channel_id) {
            let was_running = !handle.is_finished();
            handle.abort();
            was_running
        } else {
            false
        }
    }

    async fn handle_task(
        &self,
        ctx: &Context,
        target_channel_id: ChannelId,
        prompt: &str,
        existing_conv_id: Option<String>,
        workspace: PathBuf,
    ) {
        let channel_key = target_channel_id.get();
        if self.abort_channel_task(channel_key).await {
            info!(
                "채널 {} 의 이전 실행 중 작업을 중단하고 새 요청으로 교체합니다.",
                channel_key
            );
        }

        let start_time = Instant::now();
        let ws_name = workspace
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("root");
        let model_name = self.current_model.read().await.clone();

        let status_msg = match target_channel_id
            .say(
                &ctx.http,
                format!(
                    "⏳ **안티그래비티 작업 요청 중...** (📂 `{}` | 🤖 `{}`)",
                    ws_name, model_name
                ),
            )
            .await
        {
            Ok(msg) => Some(msg),
            Err(e) => {
                error!("상태 메시지 전송 실패: {:?}", e);
                None
            }
        };

        let (tx, mut rx) = mpsc::channel::<AgyProgress>(64);
        let runner = self.agy.clone();
        let prompt_string = prompt.to_string();
        let conv_clone = existing_conv_id.clone();
        let ws_clone = workspace.clone();
        let model_clone = model_name.clone();

        let agy_handle = tokio::spawn(async move {
            runner
                .execute(
                    &prompt_string,
                    conv_clone.as_deref(),
                    &ws_clone,
                    &model_clone,
                    tx,
                )
                .await
        });

        let abort_handle = agy_handle.abort_handle();
        {
            let mut map = self.active_tasks.write().await;
            map.insert(channel_key, abort_handle);
        }

        // 진행 상황 수신 루프 (상태 메시지 실시간 롤링 갱신 + 중간 답변 즉시 전송)
        let mut status_logs: Vec<String> = Vec::new();
        let mut dirty = false;
        let mut last_edit = Instant::now() - std::time::Duration::from_secs(2);
        let mut flush_ticker = tokio::time::interval(std::time::Duration::from_millis(1200));

        loop {
            tokio::select! {
                maybe_prog = rx.recv() => {
                    let Some(progress) = maybe_prog else {
                        break;
                    };
                    match progress {
                        AgyProgress::ConversationStarted(cid) => {
                            if !cid.trim().is_empty() {
                                info!("세션 등록: 채널 {} -> 세션 {}", channel_key, cid);
                                let _ = self
                                    .store
                                    .set(
                                        channel_key,
                                        SessionInfo {
                                            conversation_id: cid,
                                            workspace: workspace.clone(),
                                        },
                                    )
                                    .await;
                            }
                        }
                        AgyProgress::StatusUpdate(status) => {
                            if status_logs.last() != Some(&status) {
                                status_logs.push(status);
                                dirty = true;
                            }
                            if dirty && last_edit.elapsed().as_millis() >= 1200 {
                                if let Some(mut msg) = status_msg.clone() {
                                    let text = format_progress_status(
                                        ws_name,
                                        start_time.elapsed().as_secs(),
                                        &status_logs,
                                    );
                                    let _ = msg.edit(&ctx.http, EditMessage::new().content(text)).await;
                                }
                                last_edit = Instant::now();
                                dirty = false;
                            }
                        }
                        AgyProgress::IntermediateMessage(inter_text) => {
                            if dirty {
                                if let Some(mut msg) = status_msg.clone() {
                                    let text = format_progress_status(
                                        ws_name,
                                        start_time.elapsed().as_secs(),
                                        &status_logs,
                                    );
                                    let _ = msg.edit(&ctx.http, EditMessage::new().content(text)).await;
                                }
                                last_edit = Instant::now();
                                dirty = false;
                            }
                            send_chunked_response(ctx, target_channel_id, &inter_text).await;
                        }
                        AgyProgress::Delta(_) => {}
                    }
                }
                _ = flush_ticker.tick() => {
                    if dirty {
                        if let Some(mut msg) = status_msg.clone() {
                            let text = format_progress_status(
                                ws_name,
                                start_time.elapsed().as_secs(),
                                &status_logs,
                            );
                            let _ = msg.edit(&ctx.http, EditMessage::new().content(text)).await;
                        }
                        last_edit = Instant::now();
                        dirty = false;
                    }
                }
            }
        }

        let join_res = agy_handle.await;
        {
            let mut map = self.active_tasks.write().await;
            if let Some(existing) = map.get(&channel_key) {
                if existing.is_finished() {
                    map.remove(&channel_key);
                }
            }
        }

        match join_res {
            Ok(Ok(outcome)) => {
                let duration = start_time.elapsed().as_secs();
                if !outcome.conversation_id.trim().is_empty() {
                    let _ = self
                        .store
                        .set(
                            channel_key,
                            SessionInfo {
                                conversation_id: outcome.conversation_id.clone(),
                                workspace: workspace.clone(),
                            },
                        )
                        .await;
                }

                // 기존 상태 메시지 완료 갱신 (최근 실행 스텝 요약 유지)
                if let Some(mut msg) = status_msg {
                    let short_cid =
                        &outcome.conversation_id[..8.min(outcome.conversation_id.len())];
                    let completed_text =
                        format_completed_status(ws_name, duration, short_cid, &status_logs);
                    let _ = msg
                        .edit(&ctx.http, EditMessage::new().content(completed_text))
                        .await;
                }

                // 답변 1800자 단위 분할 전송
                send_chunked_response(ctx, target_channel_id, &outcome.response).await;
            }
            Ok(Err(e)) => {
                error!("AGY 실행 에러: {:?}", e);
                if let Some(mut msg) = status_msg {
                    let _ = msg
                        .edit(
                            &ctx.http,
                            EditMessage::new().content(format!("❌ **작업 중단/오류**: {}", e)),
                        )
                        .await;
                } else {
                    let _ = target_channel_id
                        .say(&ctx.http, format!("❌ **오류 발생**: {}", e))
                        .await;
                }
            }
            Err(e) if e.is_cancelled() => {
                info!(
                    "채널 {} 작업이 사용자 요청으로 즉시 취소되었습니다.",
                    channel_key
                );
                if let Some(mut msg) = status_msg {
                    let _ = msg
                        .edit(
                            &ctx.http,
                            EditMessage::new().content("🛑 **작업이 중지되었습니다.**"),
                        )
                        .await;
                }
            }
            Err(e) => {
                error!("태스크 조인 에러: {:?}", e);
                let _ = target_channel_id
                    .say(&ctx.http, "❌ **내부 런타임 오류가 발생했습니다.**")
                    .await;
            }
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!(
            "🤖 Antigravity Discord Remote 연결 완료: {}",
            ready.user.name
        );
        ctx.set_activity(Some(ActivityData::watching("Antigravity Remote ( !help )")));
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim();
        info!(
            "📩 [Discord Event] 작성자: {} (ID: {}), 채널: {}, 본문: {:?}",
            msg.author.name,
            msg.author.id.get(),
            msg.channel_id.get(),
            content
        );

        // 보안 화이트리스트 검사
        if let Some(allowed_id) = self.config.allowed_user_id {
            if msg.author.id.get() != allowed_id {
                warn!("⚠️ 비허용 사용자 접근 차단: {}", msg.author.id.get());
                return;
            }
        }

        // 0. 작업 즉시 중지 명령 (!stop, !중지, 멈춰, 멈춰봐)
        if is_stop_command(content) {
            let stopped = self.abort_channel_task(msg.channel_id.get()).await;
            let reply_text = if stopped {
                "🛑 **진행 중인 안티그래비티 작업을 즉시 강제 종료했습니다.** (추가 메시지 전송 차단 완료)"
            } else {
                "ℹ️ 현재 이 채널/스레드에서 실행 중인 작업이 없습니다."
            };
            let _ = msg.reply(&ctx.http, reply_text).await;
            return;
        }

        // 1. 도움말 명령
        if content == "!help" || content == "!도움말" {
            let help_embed = CreateEmbed::new()
                .title("🚀 Antigravity Discord Remote 도움말")
                .description(
                    "로컬 안티그래비티(AGY) 에이전트를 모바일/외부 디스코드에서 원격 제어합니다.",
                )
                .field(
                    "!usage (또는 !quota, !사용량)",
                    "모델별 주간 / 5시간 쿼터 잔여량 및 리셋 시간을 즉시 확인합니다.",
                    false,
                )
                .field(
                    "!stop (또는 멈춰, 멈춰봐)",
                    "현재 스레드에서 실행 중인 AGY 프로세스를 즉시 강제 종료합니다.",
                    false,
                )
                .field(
                    "!model [모델ID]",
                    "현재 모델을 조회하거나 변경합니다. (예: `!model gemini-4-argon-medium`)",
                    false,
                )
                .field(
                    "!task <내용> 또는 !ask <내용>",
                    "새 안티그래비티 작업을 시작합니다. (스레드 자동 생성)",
                    false,
                )
                .field(
                    "!task [경로] <내용>",
                    "특정 작업공간을 지정하여 시작합니다. (예: `!task [c:\\bloger] 분석해줘`)",
                    false,
                )
                .field(
                    "스레드 및 DM 내부 대화",
                    "스레드나 1:1 DM에서는 명령어 접두사 없이 바로 대화하면 맥락을 유지합니다.",
                    false,
                )
                .field(
                    "!ws [새로운 경로]",
                    "현재 작업 공간을 조회하거나 즉시 변경합니다. (예: `!ws c:\\bloger`)",
                    false,
                )
                .field(
                    "!status",
                    "현재 연결된 런타임 및 작업 공간 상태를 확인합니다.",
                    false,
                )
                .color(0x4285F4);

            let _ = msg
                .channel_id
                .send_message(&ctx.http, CreateMessage::new().embed(help_embed))
                .await;
            return;
        }

        // 1-1. 모델 조회/변경 명령 (!model)
        if content == "!model" {
            let current = self.current_model.read().await;
            let _ = msg
                .reply(
                    &ctx.http,
                    format!(
                        "🤖 현재 기본 모델: `{}`\n변경하시려면: `!model gemini-4-argon-medium`",
                        current
                    ),
                )
                .await;
            return;
        }

        if content.starts_with("!model ") {
            let new_model = content[7..].trim();
            if new_model.is_empty() {
                return;
            }
            {
                let mut m_lock = self.current_model.write().await;
                *m_lock = new_model.to_string();
            }
            let _ = msg
                .reply(
                    &ctx.http,
                    format!("✅ 기본 모델이 변경되었습니다: `{}`", new_model),
                )
                .await;
            return;
        }

        // 2. 작업 공간 변경/조회 명령 (!ws 또는 !workspace)
        if content == "!ws" || content == "!workspace" {
            let current = self.current_workspace.read().await;
            let _ = msg
                .reply(
                    &ctx.http,
                    format!(
                        "📂 현재 기본 작업 공간: `{}`\n변경하시려면: `!ws C:\\원하는\\경로`",
                        current.display()
                    ),
                )
                .await;
            return;
        }

        if content.starts_with("!ws ") || content.starts_with("!workspace ") {
            let target_path_str = if content.starts_with("!ws ") {
                &content[4..]
            } else {
                &content[11..]
            }
            .trim();

            let target_path = PathBuf::from(target_path_str);
            if !target_path.exists() {
                let _ = msg
                    .reply(
                        &ctx.http,
                        format!(
                            "❌ 해당 경로가 PC에 존재하지 않습니다: `{}`",
                            target_path.display()
                        ),
                    )
                    .await;
                return;
            }

            {
                let mut ws_lock = self.current_workspace.write().await;
                *ws_lock = target_path.clone();
            }

            let _ = msg
                .reply(
                    &ctx.http,
                    format!("✅ 작업 공간이 변경되었습니다: `{}`", target_path.display()),
                )
                .await;
            return;
        }

        // 3. 상태 확인 명령
        if content == "!status" {
            let current_ws = self.current_workspace.read().await;
            let current_model = self.current_model.read().await;
            let status_embed = CreateEmbed::new()
                .title("⚙️ Antigravity Remote 상태")
                .field(
                    "작업 공간(Workspace)",
                    format!("`{}`", current_ws.display()),
                    false,
                )
                .field("사용 모델(Model)", format!("`{}`", current_model), false)
                .field(
                    "AGY 바이너리",
                    format!("`{}`", self.config.agy_path.display()),
                    false,
                )
                .field(
                    "자동 승인(Auto Approve)",
                    if self.config.auto_approve {
                        "활성화 (자율 모드)"
                    } else {
                        "수동 검토 모드"
                    },
                    false,
                )
                .color(0x34A853);

            let _ = msg
                .channel_id
                .send_message(&ctx.http, CreateMessage::new().embed(status_embed))
                .await;
            return;
        }

        // 3-1. 사용량(쿼터) 확인 명령 (!usage, !quota, !사용량, /usage)
        if is_usage_command(content) {
            let current_ws = self.current_workspace.read().await.clone();
            match self.agy.fetch_usage(&current_ws).await {
                Ok(fields) => {
                    let mut embed = CreateEmbed::new()
                        .title("📊 Antigravity 쿼터 사용량")
                        .color(0x4285F4);
                    for (group_name, value) in fields {
                        embed = embed.field(group_name, value, false);
                    }
                    let _ = msg
                        .channel_id
                        .send_message(&ctx.http, CreateMessage::new().embed(embed))
                        .await;
                }
                Err(e) => {
                    error!("사용량 조회 에러: {:?}", e);
                    let _ = msg
                        .reply(&ctx.http, format!("❌ **사용량 조회 실패**: {}", e))
                        .await;
                }
            }
            return;
        }

        // 채널 유형 판별
        let is_dm = msg.guild_id.is_none();
        let channel_info = msg.channel_id.to_channel(&ctx.http).await;
        let is_thread = match &channel_info {
            Ok(Channel::Guild(gc)) => {
                gc.kind == ChannelType::PublicThread || gc.kind == ChannelType::PrivateThread
            }
            _ => false,
        };

        // 4. 프롬프트 추출: !task, !new, !ask, 봇 멘션(@), DM, 또는 스레드 내 대화
        let mut prompt_opt: Option<&str> = None;

        if content.starts_with("!task ") {
            prompt_opt = Some(content[6..].trim());
        } else if content.starts_with("!new ") {
            prompt_opt = Some(content[5..].trim());
        } else if content.starts_with("!ask ") {
            prompt_opt = Some(content[5..].trim());
        } else if content.starts_with("<@1547649776110276668>") {
            prompt_opt = Some(content[22..].trim());
        } else if content.starts_with("<@!1547649776110276668>") {
            prompt_opt = Some(content[23..].trim());
        } else if is_thread || is_dm {
            prompt_opt = Some(content);
        }

        if let Some(raw_prompt) = prompt_opt {
            if raw_prompt.is_empty() {
                let _ = msg
                    .reply(
                        &ctx.http,
                        "⚠️ 작업 내용을 입력해 주세요. 예: `!task main.rs 파일 분석해줘`",
                    )
                    .await;
                return;
            }

            // 특정 워크스페이스 오버라이드 문법 지원: `!task [c:\bloger] 내용`
            let (target_workspace, prompt) = if raw_prompt.starts_with('[') {
                if let Some(bracket_end) = raw_prompt.find(']') {
                    let custom_path = raw_prompt[1..bracket_end].trim();
                    let actual_prompt = raw_prompt[bracket_end + 1..].trim();
                    let p = PathBuf::from(custom_path);
                    if p.exists() {
                        (p, actual_prompt)
                    } else {
                        let default_ws = self.current_workspace.read().await.clone();
                        (default_ws, raw_prompt)
                    }
                } else {
                    let default_ws = self.current_workspace.read().await.clone();
                    (default_ws, raw_prompt)
                }
            } else {
                let default_ws = self.current_workspace.read().await.clone();
                (default_ws, raw_prompt)
            };

            if is_thread || is_dm {
                let existing_session = self.store.get(msg.channel_id.get()).await;
                let existing_cid = existing_session.as_ref().map(|s| s.conversation_id.clone());
                let ws = existing_session
                    .map(|s| s.workspace)
                    .unwrap_or(target_workspace);
                self.handle_task(&ctx, msg.channel_id, prompt, existing_cid, ws)
                    .await;
            } else {
                // 일반 채널에서는 새 스레드 생성 후 진행
                let ws_name = target_workspace
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("root");
                let thread_title = if prompt.chars().count() > 25 {
                    let truncated: String = prompt.chars().take(25).collect();
                    format!("[{}] {}...", ws_name, truncated)
                } else {
                    format!("[{}] {}", ws_name, prompt)
                };

                let thread_builder = CreateThread::new(thread_title);
                match msg
                    .channel_id
                    .create_thread_from_message(&ctx.http, msg.id, thread_builder)
                    .await
                {
                    Ok(thread_channel) => {
                        self.handle_task(&ctx, thread_channel.id, prompt, None, target_workspace)
                            .await;
                    }
                    Err(e) => {
                        warn!("스레드 생성 실패: {:?}, 일반 채널에서 진행합니다.", e);
                        self.handle_task(&ctx, msg.channel_id, prompt, None, target_workspace)
                            .await;
                    }
                }
            }
            return;
        }

        // 일반 채널에서 미인식 명령어 입력 시 안내
        if content.starts_with('!') {
            let _ = msg
                .reply(
                    &ctx.http,
                    "⚠️ 알 수 없는 명령어입니다. `!task <내용>` 또는 `!ask <내용>`으로 입력해 주세요. (`!help` 로 안내 확인)",
                )
                .await;
        }
    }
}

/// 디스코드 2000자 제한을 초과하지 않도록 1800자 단위로 깔끔하게 청킹하여 전송
async fn send_chunked_response(ctx: &Context, channel_id: ChannelId, response: &str) {
    if response.trim().is_empty() {
        return;
    }

    let max_len = 1800;
    let chars: Vec<char> = response.chars().collect();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + max_len).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        if let Err(e) = channel_id.say(&ctx.http, &chunk).await {
            error!("답변 전송 실패: {:?}", e);
            break;
        }
        start = end;
    }
}

pub fn is_usage_command(content: &str) -> bool {
    matches!(
        content.trim(),
        "!usage" | "!quota" | "!사용량" | "/usage" | "/quota"
    )
}

pub fn is_stop_command(content: &str) -> bool {
    matches!(
        content.trim(),
        "!stop" | "!중지" | "!취소" | "멈춰" | "멈춰봐" | "스톱" | "중지"
    )
}

pub fn format_progress_status(ws_name: &str, elapsed_secs: u64, logs: &[String]) -> String {
    let header = format!(
        "⏳ **안티그래비티 작업 진행 중...** (📂 `{}` | {}초 경과)",
        ws_name, elapsed_secs
    );
    if logs.is_empty() {
        return header;
    }
    let start = logs.len().saturating_sub(6);
    let body: String = logs[start..]
        .iter()
        .map(|l| format!("> {}", l))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n{}", header, body)
}

pub fn format_completed_status(
    ws_name: &str,
    duration_secs: u64,
    short_cid: &str,
    logs: &[String],
) -> String {
    let header = format!(
        "✅ **작업 완료** (소요: {}초 | 📂 `{}` | 세션: `{}`)",
        duration_secs, ws_name, short_cid
    );
    let filtered: Vec<&String> = logs
        .iter()
        .filter(|l| l.as_str() != "🧠 생각하는 중...")
        .collect();
    if filtered.is_empty() {
        return header;
    }
    let start = filtered.len().saturating_sub(6);
    let body: String = filtered[start..]
        .iter()
        .map(|l| format!("> {}", l))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n{}", header, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_usage_commands() {
        assert!(is_usage_command("!usage"));
        assert!(is_usage_command("!quota"));
        assert!(is_usage_command("!사용량"));
        assert!(is_usage_command("/usage"));
        assert!(!is_usage_command("!task usage"));
    }

    #[test]
    fn recognizes_stop_commands() {
        assert!(is_stop_command("!stop"));
        assert!(is_stop_command("멈춰"));
        assert!(is_stop_command("멈춰봐"));
        assert!(!is_stop_command("멈추지마"));
    }

    #[test]
    fn formats_rolling_status_log_keeping_recent_lines() {
        let logs: Vec<String> = (1..=8)
            .map(|i| format!("🔧 `tool_{}`: step {}", i, i))
            .collect();
        let formatted = format_progress_status("utube", 15, &logs);
        assert!(formatted.contains("⏳ **안티그래비티 작업 진행 중...** (📂 `utube` | 15초 경과)"));
        assert!(!formatted.contains("tool_1"));
        assert!(!formatted.contains("tool_2"));
        assert!(formatted.contains("> 🔧 `tool_3`: step 3"));
        assert!(formatted.contains("> 🔧 `tool_8`: step 8"));
    }
}
