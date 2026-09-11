use crate::agy::{AgyProgress, AgyRunner};
use crate::config::Config;
use crate::store::{SessionInfo, SessionStore};
use serenity::all::*;
use serenity::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

pub struct Handler {
    pub config: Config,
    pub agy: AgyRunner,
    pub store: SessionStore,
    pub current_workspace: Arc<RwLock<PathBuf>>,
}

impl Handler {
    pub fn new(config: Config, agy: AgyRunner, store: SessionStore) -> Self {
        let current_workspace = Arc::new(RwLock::new(config.default_workspace.clone()));
        Self {
            config,
            agy,
            store,
            current_workspace,
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
        let start_time = Instant::now();
        let ws_name = workspace.file_name().and_then(|n| n.to_str()).unwrap_or("root");

        let status_msg = match target_channel_id
            .say(
                &ctx.http,
                format!("⏳ **안티그래비티 작업 요청 중...** (📂 `{}`)", ws_name),
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

        let agy_handle = tokio::spawn(async move {
            runner
                .execute(&prompt_string, conv_clone.as_deref(), &ws_clone, tx)
                .await
        });

        // 진행 상황 수신 루프
        while let Some(progress) = rx.recv().await {
            match progress {
                AgyProgress::ConversationStarted(cid) => {
                    info!("세션 등록: 채널 {} -> 세션 {}", target_channel_id.get(), cid);
                    let _ = self
                        .store
                        .set(
                            target_channel_id.get(),
                            SessionInfo {
                                conversation_id: cid,
                                workspace: workspace.clone(),
                            },
                        )
                        .await;
                }
                AgyProgress::StatusUpdate(status) => {
                    if let Some(mut msg) = status_msg.clone() {
                        let _ = msg
                            .edit(&ctx.http, EditMessage::new().content(format!("⏳ {}", status)))
                            .await;
                    }
                }
                AgyProgress::Delta(_) => {}
            }
        }

        match agy_handle.await {
            Ok(Ok(outcome)) => {
                let duration = start_time.elapsed().as_secs();
                let _ = self
                    .store
                    .set(
                        target_channel_id.get(),
                        SessionInfo {
                            conversation_id: outcome.conversation_id.clone(),
                            workspace: workspace.clone(),
                        },
                    )
                    .await;

                // 기존 상태 메시지 완료 갱신
                if let Some(mut msg) = status_msg {
                    let _ = msg
                        .edit(
                            &ctx.http,
                            EditMessage::new().content(format!(
                                "✅ **작업 완료** (소요: {}초 | 📂 `{}` | 세션: `{}`)",
                                duration,
                                ws_name,
                                &outcome.conversation_id[..8.min(outcome.conversation_id.len())]
                            )),
                        )
                        .await;
                }

                // 답변 1800자 단위 분할 전송
                send_chunked_response(ctx, target_channel_id, &outcome.response).await;
            }
            Ok(Err(e)) => {
                error!("AGY 실행 에러: {:?}", e);
                let _ = target_channel_id
                    .say(&ctx.http, format!("❌ **오류 발생**: {}", e))
                    .await;
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
        info!("🤖 Antigravity Discord Remote 연결 완료: {}", ready.user.name);
        ctx.set_activity(Some(ActivityData::watching("Antigravity Remote ( !help )")));
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        // 보안 화이트리스트 검사
        if let Some(allowed_id) = self.config.allowed_user_id {
            if msg.author.id.get() != allowed_id {
                return;
            }
        }

        let content = msg.content.trim();

        // 1. 도움말 명령
        if content == "!help" || content == "!도움말" {
            let help_embed = CreateEmbed::new()
                .title("🚀 Antigravity Discord Remote 도움말")
                .description("로컬 안티그래비티 에이전트를 모바일/외부 디스코드에서 원격 제어합니다.")
                .field("!task <내용>", "새 안티그래비티 작업을 시작합니다. (스레드 자동 생성)", false)
                .field("!task [경로] <내용>", "특정 작업공간을 지정하여 시작합니다. (예: `!task [c:\\bloger] 분석해줘`)", false)
                .field("스레드 내부 대화", "생성된 스레드 안에서 대화하면 이전 작업 맥락과 워크스페이스를 유지합니다.", false)
                .field("!ws [새로운 경로]", "현재 작업 공간을 조회하거나 즉시 변경합니다. (예: `!ws c:\\bloger`)", false)
                .field("!status", "현재 연결된 런타임 및 작업 공간 상태를 확인합니다.", false)
                .color(0x4285F4);

            let _ = msg
                .channel_id
                .send_message(&ctx.http, CreateMessage::new().embed(help_embed))
                .await;
            return;
        }

        // 2. 작업 공간 변경/조회 명령 (!ws 또는 !workspace)
        if content == "!ws" || content == "!workspace" {
            let current = self.current_workspace.read().await;
            let _ = msg
                .reply(
                    &ctx.http,
                    format!("📂 현재 기본 작업 공간: `{}`\n변경하시려면: `!ws C:\\원하는\\경로`", current.display()),
                )
                .await;
            return;
        }

        if content.starts_with("!ws ") || content.starts_with("!workspace ") {
            let target_path_str = if content.starts_with("!ws ") {
                &content[4..]
            } else {
                &content[11..]
            }.trim();

            let target_path = PathBuf::from(target_path_str);
            if !target_path.exists() {
                let _ = msg
                    .reply(
                        &ctx.http,
                        format!("❌ 해당 경로가 PC에 존재하지 않습니다: `{}`", target_path.display()),
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
            let status_embed = CreateEmbed::new()
                .title("⚙️ Antigravity Remote 상태")
                .field("작업 공간(Workspace)", format!("`{}`", current_ws.display()), false)
                .field("AGY 바이너리", format!("`{}`", self.config.agy_path.display()), false)
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

        // 4. !task 또는 !new 로 새 작업 시작
        if content.starts_with("!task ") || content.starts_with("!new ") {
            let raw_prompt = if content.starts_with("!task ") {
                &content[6..]
            } else {
                &content[5..]
            }.trim();

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

            // 현재 위치가 이미 스레드인지 확인
            let channel_info = msg.channel_id.to_channel(&ctx.http).await;
            let is_thread = match &channel_info {
                Ok(Channel::Guild(gc)) => {
                    gc.kind == ChannelType::PublicThread || gc.kind == ChannelType::PrivateThread
                }
                _ => false,
            };

            if is_thread {
                let existing_session = self.store.get(msg.channel_id.get()).await;
                let existing_cid = existing_session.as_ref().map(|s| s.conversation_id.clone());
                let ws = existing_session
                    .map(|s| s.workspace)
                    .unwrap_or(target_workspace);
                self.handle_task(&ctx, msg.channel_id, prompt, existing_cid, ws).await;
            } else {
                // 새 스레드 생성
                let ws_name = target_workspace.file_name().and_then(|n| n.to_str()).unwrap_or("root");
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
                        self.handle_task(&ctx, thread_channel.id, prompt, None, target_workspace).await;
                    }
                    Err(e) => {
                        warn!("스레드 생성 실패: {:?}, 일반 채널에서 진행합니다.", e);
                        self.handle_task(&ctx, msg.channel_id, prompt, None, target_workspace).await;
                    }
                }
            }
            return;
        }

        // 5. 스레드 내부에서 일반 메시지 전송 시 (이어서 대화하기)
        let channel_info = msg.channel_id.to_channel(&ctx.http).await;
        let is_thread = match &channel_info {
            Ok(Channel::Guild(gc)) => {
                gc.kind == ChannelType::PublicThread || gc.kind == ChannelType::PrivateThread
            }
            _ => false,
        };

        if is_thread {
            if let Some(existing_session) = self.store.get(msg.channel_id.get()).await {
                // 이 스레드는 안티그래비티 세션과 연결되어 있음! 해당 워크스페이스 유지하여 실행
                self.handle_task(
                    &ctx,
                    msg.channel_id,
                    content,
                    Some(existing_session.conversation_id),
                    existing_session.workspace,
                )
                .await;
            }
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
