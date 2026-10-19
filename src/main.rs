mod agy;
mod config;
mod discord;
mod store;

use agy::AgyRunner;
use config::Config;
use discord::Handler;
use serenity::prelude::*;
use store::SessionStore;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agy_discord_remote=info,serenity=warn".into()),
        )
        .init();

    info!("🚀 Antigravity Discord Remote (Rust) 시작 중...");

    let config = Config::load()?;
    info!("📂 기본 작업 공간: {:?}", config.default_workspace);
    info!("🤖 AGY 바이너리: {:?}", config.agy_path);
    if let Some(user_id) = config.allowed_user_id {
        info!("🔒 보안 화이트리스트 적용 (User ID: {})", user_id);
    } else {
        info!("🔓 화이트리스트 미적용 (모든 사용자 메시지 허용)");
    }

    let agy_runner = AgyRunner::new(config.agy_path.clone(), config.auto_approve);
    let store = SessionStore::new("sessions.json");

    let handler = Handler::new(config.clone(), agy_runner, store);

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&config.discord_token, intents)
        .event_handler(handler)
        .await?;

    info!("🌐 디스코드 게이트웨이 연결 대기 중...");
    client.start().await?;

    Ok(())
}
