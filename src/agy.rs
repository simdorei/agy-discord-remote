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
    Init { conversation_id: String },
    #[serde(rename = "step_update")]
    StepUpdate { step_update: RawStepUpdate },
    #[serde(rename = "result")]
    Result { result: RawResult },
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
    IntermediateMessage(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptStep {
    pub step_index: u64,
    pub status_lines: Vec<String>,
    pub intermediate_message: Option<String>,
    pub final_candidate_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTranscriptLine {
    step_index: Option<u64>,
    #[serde(rename = "type")]
    step_type: Option<String>,
    thinking: Option<String>,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RawToolCall>,
}

#[derive(Debug, Deserialize)]
struct RawToolCall {
    name: String,
    #[serde(default)]
    args: serde_json::Map<String, serde_json::Value>,
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", taken)
    } else {
        taken
    }
}

fn clean_json_arg(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => {
            serde_json::from_str::<String>(s).unwrap_or_else(|_| s.trim_matches('"').to_string())
        }
        other => other.to_string(),
    }
}

pub fn parse_transcript_step(raw_line: &str) -> Option<TranscriptStep> {
    let parsed: RawTranscriptLine = serde_json::from_str(raw_line).ok()?;
    if parsed.step_type.as_deref() != Some("PLANNER_RESPONSE") {
        return None;
    }
    let step_index = parsed.step_index?;
    let mut status_lines = Vec::new();

    if let Some(thinking) = parsed.thinking {
        if let Some(first_line) = thinking.lines().map(str::trim).find(|l| !l.is_empty()) {
            status_lines.push(format!("💭 {}", truncate_chars(first_line, 90)));
        }
    }

    for tc in &parsed.tool_calls {
        let desc = ["toolAction", "toolSummary", "CommandLine", "AbsolutePath"]
            .iter()
            .find_map(|k| tc.args.get(*k))
            .map(clean_json_arg)
            .unwrap_or_default();
        let desc_first_line = desc.lines().next().unwrap_or("").trim();

        let icon = match tc.name.as_str() {
            "run_command" | "manage_task" => "⚡",
            "view_file" | "read_url_content" | "search_web" => "🔍",
            "replace_file_content" | "write_to_file" => "✏️",
            "invoke_subagent" | "send_message" => "🤖",
            _ => "🔧",
        };

        if desc_first_line.is_empty() {
            status_lines.push(format!("{} `{}`", icon, tc.name));
        } else {
            status_lines.push(format!(
                "{} `{}`: {}",
                icon,
                tc.name,
                truncate_chars(desc_first_line, 80)
            ));
        }
    }

    let clean_content = parsed
        .content
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());

    let (intermediate_message, final_candidate_message) = match clean_content {
        Some(text) if !parsed.tool_calls.is_empty() => (Some(text), None),
        Some(text) => (None, Some(text)),
        None => (None, None),
    };

    Some(TranscriptStep {
        step_index,
        status_lines,
        intermediate_message,
        final_candidate_message,
    })
}

fn transcript_path_for(conv_id: &str) -> Option<PathBuf> {
    if conv_id.trim().is_empty() {
        return None;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(
        PathBuf::from(home)
            .join(".gemini")
            .join("antigravity-cli")
            .join("brain")
            .join(conv_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl"),
    )
}

fn max_step_index_in_file(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    content
        .lines()
        .rev()
        .filter_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()?
                .get("step_index")?
                .as_u64()
        })
        .next()
}

async fn poll_transcript_updates(
    conv_id: &str,
    last_seen_step: &mut Option<u64>,
    pending_final_candidate: &mut Option<String>,
    progress_tx: &Sender<AgyProgress>,
) {
    let Some(path) = transcript_path_for(conv_id) else {
        return;
    };
    let Ok(content) = tokio::fs::read_to_string(&path).await else {
        return;
    };

    for line in content.lines() {
        let Some(step) = parse_transcript_step(line) else {
            continue;
        };
        if last_seen_step.is_some_and(|last| step.step_index <= last) {
            continue;
        }
        *last_seen_step = Some(step.step_index);

        if (!step.status_lines.is_empty()
            || step.intermediate_message.is_some()
            || step.final_candidate_message.is_some())
            && pending_final_candidate.is_some()
        {
            if let Some(prev_msg) = pending_final_candidate.take() {
                let _ = progress_tx
                    .send(AgyProgress::IntermediateMessage(prev_msg))
                    .await;
            }
        }

        for status_line in step.status_lines {
            let _ = progress_tx
                .send(AgyProgress::StatusUpdate(status_line))
                .await;
        }

        if let Some(msg) = step.intermediate_message {
            let _ = progress_tx
                .send(AgyProgress::IntermediateMessage(msg))
                .await;
        }

        if let Some(cand) = step.final_candidate_message {
            *pending_final_candidate = Some(cand);
        }
    }
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
        model: &str,
        progress_tx: Sender<AgyProgress>,
    ) -> Result<AgyOutcome> {
        let mut last_seen_step: Option<u64> = existing_conv_id
            .and_then(transcript_path_for)
            .and_then(|p| max_step_index_in_file(&p));
        let mut pending_final_candidate: Option<String> = None;

        let mut cmd = Command::new(&self.agy_path);

        cmd.arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--add-dir")
            .arg(workspace)
            .current_dir(workspace);

        if !model.trim().is_empty() {
            cmd.arg("--model").arg(model.trim());
        }

        if let Some(cid) = existing_conv_id {
            cmd.arg("--conversation").arg(cid);
        }

        if self.auto_approve {
            cmd.arg("--dangerously-skip-permissions");
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        info!(
            "AGY 실행: {:?} (model: {}, workspace: {:?}, conv: {:?})",
            self.agy_path, model, workspace, existing_conv_id
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
        let mut saw_result_event = false;

        let mut reader = BufReader::new(stdout).lines();
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(800));

        loop {
            tokio::select! {
                line_res = reader.next_line() => {
                    let Ok(Some(line)) = line_res else {
                        break;
                    };
                    let line_trimmed = line.trim();
                    if line_trimmed.is_empty() {
                        continue;
                    };

                    match serde_json::from_str::<RawAgyEvent>(line_trimmed) {
                        Ok(RawAgyEvent::Init { conversation_id }) => {
                            if conv_id.is_empty() {
                                conv_id = conversation_id.clone();
                            } else {
                                update_conv_id_if_present(&mut conv_id, Some(conversation_id.clone()));
                            }
                            let _ = progress_tx
                                .send(AgyProgress::ConversationStarted(conv_id.clone()))
                                .await;
                        }
                        Ok(RawAgyEvent::StepUpdate { step_update }) => {
                            if conv_id.is_empty() {
                                update_conv_id_if_present(&mut conv_id, step_update.conversation_id);
                            }

                            if let Some(delta) = step_update.text_delta {
                                accumulated_response.push_str(&delta);
                                let _ = progress_tx.send(AgyProgress::Delta(delta)).await;
                            }

                            if step_update.step_type.as_deref() == Some("user_input") {
                                let _ = progress_tx
                                    .send(AgyProgress::StatusUpdate("🧠 생각하는 중...".to_string()))
                                    .await;
                            }

                            poll_transcript_updates(
                                &conv_id,
                                &mut last_seen_step,
                                &mut pending_final_candidate,
                                &progress_tx,
                            )
                            .await;
                        }
                        Ok(RawAgyEvent::Result { result }) => {
                            saw_result_event = true;
                            update_conv_id_if_present(&mut conv_id, result.conversation_id);
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
                _ = ticker.tick() => {
                    poll_transcript_updates(
                        &conv_id,
                        &mut last_seen_step,
                        &mut pending_final_candidate,
                        &progress_tx,
                    )
                    .await;
                }
            }
        }

        poll_transcript_updates(
            &conv_id,
            &mut last_seen_step,
            &mut pending_final_candidate,
            &progress_tx,
        )
        .await;

        let status = child.wait().await?;
        if !status.success() {
            warn!("agy 프로세스 비정상 종료 코드: {:?}", status.code());
            if !saw_result_event && accumulated_response.trim().is_empty() {
                anyhow::bail!(
                    "AGY 프로세스가 비정상 종료되었습니다 (종료 코드: {:?} — 쿼터 소진 또는 중단)",
                    status.code()
                );
            }
        }

        Ok(AgyOutcome {
            conversation_id: conv_id,
            response: accumulated_response,
            status: final_status,
        })
    }

    pub async fn fetch_usage(&self, workspace: &Path) -> Result<Vec<(String, String)>> {
        let mut cmd = Command::new(&self.agy_path);
        cmd.arg("-p")
            .arg("/usage")
            .arg("--output-format")
            .arg("json")
            .current_dir(workspace);

        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        let output = cmd
            .output()
            .await
            .with_context(|| format!("agy.exe /usage 실행 실패: {:?}", self.agy_path))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        format_usage_json(stdout.trim())
    }
}

pub fn update_conv_id_if_present(conv_id: &mut String, candidate: Option<String>) {
    if let Some(cid) = candidate {
        if !cid.trim().is_empty() {
            *conv_id = cid;
        }
    }
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    response: Option<String>,
    command: Option<UsageCommand>,
}

#[derive(Debug, Deserialize)]
struct UsageCommand {
    data: Option<UsageData>,
}

#[derive(Debug, Deserialize)]
struct UsageData {
    groups: Option<Vec<UsageGroup>>,
}

#[derive(Debug, Deserialize)]
struct UsageGroup {
    name: String,
    buckets: Vec<UsageBucket>,
}

#[derive(Debug, Deserialize)]
struct UsageBucket {
    name: String,
    remaining_fraction: f64,
    reset_time: Option<String>,
}

fn parse_utc_to_unix(iso: &str) -> Option<i64> {
    let s = iso.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next()?.split('.').next()?.parse().ok()?;

    let y_adj = y - if m <= 2 { 1 } else { 0 };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

pub fn format_usage_json(raw_json: &str) -> Result<Vec<(String, String)>> {
    let parsed: UsageResponse = serde_json::from_str(raw_json).context("사용량 JSON 파싱 실패")?;

    if let Some(groups) = parsed.command.and_then(|c| c.data).and_then(|d| d.groups) {
        if !groups.is_empty() {
            let mut fields = Vec::with_capacity(groups.len());
            for g in groups {
                let mut lines = Vec::with_capacity(g.buckets.len());
                for b in g.buckets {
                    let pct = (b.remaining_fraction * 100.0).round() as i64;
                    let reset_str = match b.reset_time.as_deref() {
                        Some(iso) => match parse_utc_to_unix(iso) {
                            Some(ts) => format!("리셋: <t:{}:R> (<t:{}:t>)", ts, ts),
                            None => format!("리셋: `{}`", iso),
                        },
                        None => "리셋 정보 없음".to_string(),
                    };
                    lines.push(format!("• **{}**: **{}%** ({})", b.name, pct, reset_str));
                }
                fields.push((g.name, lines.join("\n")));
            }
            return Ok(fields);
        }
    }

    let fallback = parsed
        .response
        .unwrap_or_else(|| "사용량 데이터가 비어 있습니다.".to_string());
    Ok(vec![("Quota".to_string(), fallback)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_json_with_discord_timestamps() {
        let sample = r#"{"conversation_id":"","status":"SUCCESS","response":"Gemini Models\tWeekly Limit Remaining\t38%\t2026-10-14T05:41:33Z\n","command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"id":"gemini-weekly","name":"Weekly Limit Remaining","remaining_fraction":0.3754,"reset_time":"2026-10-14T05:41:33Z"},{"id":"gemini-5h","name":"Five Hour Limit Remaining","remaining_fraction":0.9203,"reset_time":"2026-10-13T22:44:26Z"}]}]}}}"#;
        let fields = format_usage_json(sample).expect("should parse usage json");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "Gemini Models");
        assert!(fields[0].1.contains("38%"));
        assert!(fields[0].1.contains("92%"));
        assert!(fields[0].1.contains("<t:1791956493:R>"));
    }

    #[test]
    fn ignores_empty_conversation_id_on_result() {
        let mut conv_id = "existing-session-id".to_string();
        update_conv_id_if_present(&mut conv_id, Some("".to_string()));
        assert_eq!(conv_id, "existing-session-id");
    }

    #[test]
    fn extracts_thinking_and_tool_calls_from_planner_response() {
        let line = r#"{"step_index":171,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","thinking":"Observing the current conversation's transcript file.\nMore details here.","tool_calls":[{"name":"view_file","args":{"AbsolutePath":"\"C:\\\\utube\\\\src\\\\agy.rs\"","toolAction":"\"Viewing src/agy.rs\"","toolSummary":"\"View src/agy.rs\""}},{"name":"run_command","args":{"CommandLine":"\"cargo test --release\"","toolAction":"\"Running cargo test\""}}]}"#;
        let step = parse_transcript_step(line).expect("should parse planner response");
        assert_eq!(step.step_index, 171);
        assert_eq!(step.intermediate_message, None);
        assert_eq!(step.final_candidate_message, None);
        assert_eq!(step.status_lines.len(), 3);
        assert_eq!(
            step.status_lines[0],
            "💭 Observing the current conversation's transcript file."
        );
        assert_eq!(step.status_lines[1], "🔍 `view_file`: Viewing src/agy.rs");
        assert_eq!(step.status_lines[2], "⚡ `run_command`: Running cargo test");
    }

    #[test]
    fn distinguishes_intermediate_content_from_final_candidate() {
        let intermediate_line = r#"{"step_index":10,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"파일을 먼저 확인하겠습니다.","tool_calls":[{"name":"view_file","args":{"toolAction":"\"Viewing file\""}}]}"#;
        let step1 = parse_transcript_step(intermediate_line).expect("should parse intermediate");
        assert_eq!(
            step1.intermediate_message.as_deref(),
            Some("파일을 먼저 확인하겠습니다.")
        );
        assert_eq!(step1.final_candidate_message, None);

        let final_line = r#"{"step_index":12,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"최종 답변입니다."}"#;
        let step2 = parse_transcript_step(final_line).expect("should parse final");
        assert_eq!(step2.intermediate_message, None);
        assert_eq!(
            step2.final_candidate_message.as_deref(),
            Some("최종 답변입니다.")
        );
    }
}
