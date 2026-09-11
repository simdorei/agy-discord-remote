use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

#[derive(Debug, Deserialize)]
#[serde(tag = "event")]
pub enum RawAgyEvent {
    #[serde(rename = "init")]
    Init {
        conversation_id: String,
    },
    #[serde(rename = "step_update")]
    StepUpdate {
        step_update: RawStepUpdate,
    },
    #[serde(rename = "result")]
    Result {
        result: RawResult,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub struct RawStepUpdate {
    pub conversation_id: Option<String>,
    pub step_type: Option<String>,
    pub state: Option<String>,
    pub text_delta: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RawResult {
    pub conversation_id: Option<String>,
    pub status: Option<String>,
    pub response: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AgyProgress {
    ConversationStarted(String),
    Delta(String),
    StatusUpdate(String),
}

#[derive(Debug, Clone)]
pub struct AgyOutcome {
    pub conversation_id: String,
    pub response: String,
    pub status: String,
}

#[derive(Clone)]
pub struct AgyRunner {
    pub agy_path: PathBuf,
    pub auto_approve: bool,
}

impl AgyRunner {
    pub fn new(agy_path: PathBuf, auto_approve: bool) -> Self {
        Self {
            agy_path,
            auto_approve,
        }
    }

    pub async fn execute(
        &self,
        prompt: &str,
        existing_conv_id: Option<&str>,
        workspace: &Path,
        progress_tx: Sender<AgyProgress>,
    ) -> Result<AgyOutcome> {
        let mut cmd = Command::new(&self.agy_path);

        cmd.arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--add-dir")
            .arg(workspace)
            .current_dir(workspace);

        if let Some(cid) = existing_conv_id {
            cmd.arg("--conversation").arg(cid);
        }

        if self.auto_approve {
            cmd.arg("--dangerously-skip-permissions");
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        info!(
            "AGY 실행: {:?} (workspace: {:?}, conv: {:?})",
            self.agy_path, workspace, existing_conv_id
        );

        let mut child = cmd
            .spawn()
            .with_context(|| format!("agy.exe 실행 실패: {:?}", self.agy_path))?;

        let stdout = child.stdout.take().context("stdout 캡처 실패")?;
        let stderr = child.stderr.take().context("stderr 캡처 실패")?;

        // Stderr 비동기 로깅
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if !line.trim().is_empty() {
                    debug!("[AGY STDERR] {}", line);
                }
            }
        });

        let mut conv_id = existing_conv_id.unwrap_or("").to_string();
        let mut accumulated_response = String::new();
        let mut final_status = "SUCCESS".to_string();

        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<RawAgyEvent>(line_trimmed) {
                Ok(RawAgyEvent::Init { conversation_id }) => {
                    conv_id = conversation_id.clone();
                    let _ = progress_tx
                        .send(AgyProgress::ConversationStarted(conversation_id))
                        .await;
                }
                Ok(RawAgyEvent::StepUpdate { step_update }) => {
                    if let Some(cid) = step_update.conversation_id {
                        if conv_id.is_empty() {
                            conv_id = cid;
                        }
                    }

                    if let Some(delta) = step_update.text_delta {
                        accumulated_response.push_str(&delta);
                        let _ = progress_tx.send(AgyProgress::Delta(delta)).await;
                    }

                    if let Some(st) = step_update.step_type {
                        if st == "user_input" {
                            let _ = progress_tx
                                .send(AgyProgress::StatusUpdate("🧠 생각하는 중...".to_string()))
                                .await;
                        }
                    }
                }
                Ok(RawAgyEvent::Result { result }) => {
                    if let Some(cid) = result.conversation_id {
                        conv_id = cid;
                    }
                    if let Some(resp) = result.response {
                        accumulated_response = resp;
                    }
                    if let Some(st) = result.status {
                        final_status = st;
                    }
                }
                Err(e) => {
                    debug!("JSON 파싱 무시 (비이벤트 라인): {} ({})", line_trimmed, e);
                }
                _ => {}
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            warn!("agy 프로세스 비정상 종료 코드: {:?}", status.code());
        }

        Ok(AgyOutcome {
            conversation_id: conv_id,
            response: accumulated_response,
            status: final_status,
        })
    }
}
