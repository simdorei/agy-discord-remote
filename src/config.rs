use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub discord_token: String,
    pub allowed_user_id: Option<u64>,
    pub agy_path: PathBuf,
    pub default_workspace: PathBuf,
    pub default_model: String,
    pub auto_approve: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let _ = dotenvy::from_path(PathBuf::from(r"C:\utube\agy-discord-remote\.env"));

        let discord_token = env::var("DISCORD_BOT_TOKEN")
            .context("DISCORD_BOT_TOKEN이 .env 또는 환경변수에 설정되지 않았습니다.")?;

        let allowed_user_id = env::var("ALLOWED_USER_ID")
            .ok()
            .and_then(|id_str| id_str.trim().parse::<u64>().ok());

        let agy_path = env::var("AGY_PATH").map(PathBuf::from).unwrap_or_else(|_| {
            let default_path = PathBuf::from(r"C:\Users\ihavg\.gemini\bin\agy.exe");
            if default_path.exists() {
                default_path
            } else {
                PathBuf::from("agy.exe")
            }
        });

        let default_workspace = env::var("DEFAULT_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\utube"));

        let default_model = env::var("AGY_MODEL")
            .map(|m| m.trim().to_string())
            .ok()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "gemini-4-argon-medium".to_string());

        let auto_approve = env::var("AUTO_APPROVE")
            .map(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
            .unwrap_or(true);

        Ok(Self {
            discord_token,
            allowed_user_id,
            agy_path,
            default_workspace,
            default_model,
            auto_approve,
        })
    }
}
