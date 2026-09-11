use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub conversation_id: String,
    pub workspace: PathBuf,
}

#[derive(Clone)]
pub struct SessionStore {
    file_path: PathBuf,
    sessions: Arc<RwLock<HashMap<u64, SessionInfo>>>,
}

impl SessionStore {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let file_path = path.as_ref().to_path_buf();
        let mut map = HashMap::new();

        if file_path.exists() {
            if let Ok(content) = fs::read_to_string(&file_path) {
                if let Ok(deserialized) = serde_json::from_str::<HashMap<u64, SessionInfo>>(&content) {
                    info!("저장된 세션 {}개를 불러왔습니다.", deserialized.len());
                    map = deserialized;
                }
            }
        }

        Self {
            file_path,
            sessions: Arc::new(RwLock::new(map)),
        }
    }

    pub async fn get(&self, channel_id: u64) -> Option<SessionInfo> {
        let lock = self.sessions.read().await;
        lock.get(&channel_id).cloned()
    }

    pub async fn set(&self, channel_id: u64, info: SessionInfo) -> Result<()> {
        {
            let mut lock = self.sessions.write().await;
            lock.insert(channel_id, info);
        }
        self.save().await
    }

    async fn save(&self) -> Result<()> {
        let lock = self.sessions.read().await;
        let data = serde_json::to_string_pretty(&*lock)?;
        if let Err(e) = fs::write(&self.file_path, data) {
            warn!("세션 저장 실패: {}", e);
        }
        Ok(())
    }
}
