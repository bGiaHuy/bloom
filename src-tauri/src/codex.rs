//! Local status and explicitly requested, one-time island approval responses.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const HOOK_SCRIPT: &str = include_str!("../../scripts/codex-hook.ps1");
const HOOK_EVENTS: [&str; 8] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSession {
    id: String,
    title: String,
    project: String,
    status: String,
    detail: String,
    updated_at: u64,
    approval: Option<CodexApproval>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexApproval {
    request_id: String,
    session_id: String,
    turn_id: String,
    tool: String,
    command: String,
    expires_at: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSnapshot {
    installed: bool,
    sessions: Vec<CodexSession>,
}

fn codex_root() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODEX_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    std::env::var_os("USERPROFILE")
        .map(|path| PathBuf::from(path).join(".codex"))
        .ok_or_else(|| "Cannot locate the Codex configuration folder.".into())
}

fn state_root() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|path| PathBuf::from(path).join("bloom").join("codex"))
        .ok_or_else(|| "Cannot locate the Bloom configuration folder.".into())
}

fn hook_command(script: &std::path::Path) -> String {
    format!("powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\"", script.display())
}

fn hooks_installed(root: &std::path::Path, command: &str) -> bool {
    if !state_root().is_ok_and(|path| path.join("bloom-codex-hook.ps1").is_file()) {
        return false;
    }
    let Some(config) = fs::read(root.join("hooks.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return false;
    };
    HOOK_EVENTS.iter().all(|event| {
        config["hooks"][event].as_array().is_some_and(|groups| {
            groups.iter().any(|group| {
                group["hooks"].as_array().is_some_and(|hooks| {
                    hooks
                        .iter()
                        .any(|hook| hook["command"].as_str() == Some(command))
                })
            })
        })
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn clipped(value: &str, length: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(length)
        .collect()
}

// The local index supplies titles without reading the conversation transcript.
// Its format is best-effort: project + id remain useful if Codex changes it.
fn session_titles(root: &std::path::Path) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    let Ok(mut file) = File::open(root.join("session_index.jsonl")) else {
        return titles;
    };
    let Ok(metadata) = file.metadata() else {
        return titles;
    };
    let start = metadata.len().saturating_sub(1024 * 1024);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return titles;
    }
    let mut bytes = Vec::new();
    if file.take(1024 * 1024).read_to_end(&mut bytes).is_err() {
        return titles;
    }
    for line in String::from_utf8_lossy(&bytes)
        .lines()
        .skip(usize::from(start > 0))
    {
        if let Ok(row) = serde_json::from_str::<Value>(line) {
            if let (Some(id), Some(title)) = (row["id"].as_str(), row["thread_name"].as_str()) {
                titles.insert(id.into(), clipped(title, 140));
            }
        }
    }
    titles
}

fn parse_session(row: &Value, titles: &HashMap<String, String>, now: u64) -> Option<CodexSession> {
    if row["schema"].as_u64()? != 1 {
        return None;
    }
    let id = row["session_id"].as_str()?;
    if !valid_id(id) {
        return None;
    }
    let updated_at = row["updated_at"].as_u64()?;
    let age = now.saturating_sub(updated_at);
    if updated_at > now + 60_000 || age > 7 * 24 * 60 * 60 * 1000 {
        return None;
    }
    let raw_status = row["status"].as_str()?;
    if raw_status == "closed" {
        return None;
    }
    let mut status = match raw_status {
        "idle" | "running" | "awaiting_approval" | "completed" | "interrupted" => raw_status,
        _ => return None,
    };
    let mut detail = clipped(row["detail"].as_str().unwrap_or(""), 240);
    // Hooks don't heartbeat; never assert that an old process is still running.
    if status == "running" && age > 15 * 60 * 1000 {
        status = "unknown";
        detail = "No recent signal. Open Codex to check this chat.".into();
    }
    let cwd = row["cwd"]
        .as_str()
        .unwrap_or("")
        .trim_end_matches(['\\', '/']);
    let project = clipped(cwd.rsplit(['\\', '/']).next().unwrap_or(""), 80);
    let title = titles
        .get(id)
        .filter(|title| !title.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "{} · {}",
                if project.is_empty() {
                    "Codex"
                } else {
                    &project
                },
                &id[..id.len().min(8)]
            )
        });
    Some(CodexSession {
        id: id.into(),
        title,
        project,
        status: status.into(),
        detail,
        updated_at,
        approval: None,
    })
}

fn read_snapshot() -> Result<CodexSnapshot, String> {
    let root = state_root()?;
    let codex = codex_root()?;
    let installed = hooks_installed(&codex, &hook_command(&root.join("bloom-codex-hook.ps1")));
    if !installed {
        return Ok(CodexSnapshot {
            installed,
            sessions: Vec::new(),
        });
    }
    let titles = session_titles(&codex);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut sessions = Vec::new();
    match fs::read_dir(&root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                let Ok(file) = File::open(&path) else {
                    continue;
                };
                let mut bytes = Vec::new();
                if file.take(32 * 1024 + 1).read_to_end(&mut bytes).is_err()
                    || bytes.len() > 32 * 1024
                {
                    continue;
                }
                if let Ok(row) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(session) = parse_session(&row, &titles, now) {
                        sessions.push(session);
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("Cannot read the Codex status folder.".into()),
    }
    sessions.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "awaiting_approval" => 0,
            "running" => 1,
            _ => 2,
        };
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then(b.updated_at.cmp(&a.updated_at))
    });
    sessions.truncate(24);
    for session in &mut sessions {
        if matches!(session.status.as_str(), "running" | "awaiting_approval") {
            session.approval = active_approvals(&root, &session.id, now).into_iter().next();
            if session.approval.is_some() {
                session.status = "awaiting_approval".into();
            }
        }
    }
    Ok(CodexSnapshot {
        installed,
        sessions,
    })
}

fn active_approvals(root: &std::path::Path, id: &str, now: u64) -> Vec<CodexApproval> {
    let mut approvals = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("requests")) else {
        return approvals;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json")
            || !entry.file_type().is_ok_and(|v| v.is_file())
        {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > 64 * 1024 {
            continue;
        }
        if let Ok(request) = serde_json::from_slice::<CodexApproval>(&bytes) {
            if request.expires_at <= now
                && valid_id(&request.session_id)
                && request.request_id.len() == 32
                && request.request_id.bytes().all(|c| c.is_ascii_hexdigit())
                && path.file_name().and_then(|v| v.to_str())
                    == Some(format!("{}-{}.json", request.session_id, request.request_id).as_str())
            {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("response"));
                continue;
            }
            if request.session_id == id
                && valid_request(&request, now)
                && !path.with_extension("response").exists()
            {
                approvals.push(request);
            }
        }
    }
    approvals.sort_by_key(|r| r.expires_at);
    approvals
}

fn valid_request(request: &CodexApproval, now: u64) -> bool {
    valid_id(&request.session_id)
        && request.request_id.len() == 32
        && request.request_id.bytes().all(|c| c.is_ascii_hexdigit())
        && !request.command.is_empty()
        && request.command.len() <= 32768
        && request.expires_at > now
        && request.expires_at <= now.saturating_add(130_000)
}

#[tauri::command]
pub fn respond_codex_approval(
    id: String,
    request_id: String,
    decision: String,
) -> Result<(), String> {
    if !valid_id(&id)
        || request_id.len() != 32
        || !request_id.bytes().all(|c| c.is_ascii_hexdigit())
        || !matches!(decision.as_str(), "allow" | "deny")
    {
        return Err("Invalid approval response.".into());
    }
    let root = state_root()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let request = active_approvals(&root, &id, now)
        .into_iter()
        .find(|r| r.request_id == request_id)
        .ok_or("This request has expired. Review it in Codex.")?;
    let state: Value = serde_json::from_slice(
        &fs::read(root.join(format!("{id}.json"))).map_err(|_| "Chat is no longer available.")?,
    )
    .map_err(|_| "Chat is no longer available.")?;
    if state["status"] != "awaiting_approval" || state["turn_id"].as_str() != Some(&request.turn_id)
    {
        return Err("This request is no longer active.".into());
    }
    // Create once: repeated clicks cannot overwrite an earlier decision.
    let response = root
        .join("requests")
        .join(format!("{id}-{request_id}.response"));
    let temporary = response.with_extension("response.tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| "A response was already sent or the request expired.")?;
    let result = (|| {
        file.write_all(
            serde_json::json!({"requestId": request_id, "decision": decision})
                .to_string()
                .as_bytes(),
        )
        .map_err(|_| "Cannot send the approval response.".to_string())?;
        file.sync_all()
            .map_err(|_| "Cannot save the approval response.".to_string())?;
        drop(file);
        fs::hard_link(&temporary, &response)
            .map_err(|_| "A response was already sent or the request expired.".to_string())
    })();
    let _ = fs::remove_file(temporary);
    result
}

pub fn start_status_service(app: tauri::AppHandle) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let mut previous = String::new();
        loop {
            if crate::state::SHUTTING_DOWN.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if let Ok(root) = state_root() {
                if crate::utils::get_setting_str(&app, "bloom-codex-enabled").as_deref()
                    != Some("false")
                {
                    let _ = fs::create_dir_all(&root);
                    let _ = fs::write(root.join("island-heartbeat"), b"active");
                } else {
                    let _ = fs::remove_file(root.join("island-heartbeat"));
                }
            }
            if let Ok(root) = state_root() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let _ = active_approvals(&root, "", now);
            }
            if let Ok(snapshot) = read_snapshot() {
                let serialized = serde_json::to_string(&snapshot).unwrap_or_default();
                if serialized != previous {
                    previous = serialized;
                    let _ = app.emit("codex-update", snapshot);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if let Ok(root) = state_root() {
            let _ = fs::remove_file(root.join("island-heartbeat"));
        }
    });
}

#[tauri::command]
pub async fn get_codex_sessions() -> Result<CodexSnapshot, String> {
    tauri::async_runtime::spawn_blocking(read_snapshot)
        .await
        .map_err(|_| "Cannot read Codex status.".to_string())?
}

fn configure_bridge(install: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let root = state_root()?;
    fs::create_dir_all(&root).map_err(|_| "Cannot create the Codex status folder.")?;
    let script = root.join("bloom-codex-hook.ps1");
    fs::write(&script, HOOK_SCRIPT).map_err(|_| "Cannot save the Codex status hook.")?;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg(if install { "-Install" } else { "-Uninstall" })
        .creation_flags(0x08000000)
        .output()
        .map_err(|_| "Cannot configure Codex hooks. Check that PowerShell is available.")?;
    if !output.status.success() {
        return Err("Cannot update Codex hooks. Check hooks.json is valid and writable; existing hooks were preserved.".into());
    }
    if install {
        fs::write(root.join("island-approvals-enabled"), b"enabled")
            .map_err(|_| "Cannot enable island approvals.")?;
    } else {
        let _ = fs::remove_file(root.join("island-approvals-enabled"));
    }
    Ok(())
}

fn read_rate_limits() -> Result<Value, String> {
    use std::os::windows::process::CommandExt;
    let cli = if let Some(path) = std::env::var_os("CODEX_CLI_PATH") {
        PathBuf::from(path)
    } else {
        let output = std::process::Command::new("powershell.exe").args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "(Get-AppxPackage -Name OpenAI.Codex | Select-Object -First 1 -ExpandProperty InstallLocation)"]).creation_flags(0x08000000).output().map_err(|_| "Cannot find Codex.")?;
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
            .join("app")
            .join("resources")
            .join("codex.exe")
    };
    if !cli.is_file() {
        return Err("Codex desktop is required to read usage limits.".into());
    }
    let mut child = std::process::Command::new(cli)
        .args(["app-server", "--stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "Cannot read Codex usage.")?;
    let stdout = child.stdout.take().ok_or("Cannot read Codex usage.")?;
    let mut stdin = child.stdin.take().ok_or("Cannot read Codex usage.")?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut bytes = Vec::new();
            if reader
                .by_ref()
                .take(1024 * 1024)
                .read_until(b'\n', &mut bytes)
                .unwrap_or(0)
                == 0
                || bytes.len() >= 1024 * 1024
            {
                break;
            }
            if let Ok(row) = serde_json::from_slice::<Value>(&bytes) {
                if send.send(row).is_err() {
                    break;
                }
            }
        }
    });
    let result = (|| {
        writeln!(stdin, "{}", serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"bloom_usage","version":"1.0"}}})).map_err(|_| "Cannot read Codex usage.")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let row = receive
                .recv_timeout(remaining)
                .map_err(|_| "Codex usage request timed out.")?;
            if row["id"] == 1 {
                if row.get("error").is_some() {
                    return Err("Cannot initialize Codex usage reader.".into());
                }
                writeln!(stdin, "{{\"method\":\"initialized\"}}")
                    .map_err(|_| "Cannot read Codex usage.")?;
                writeln!(stdin, "{{\"id\":2,\"method\":\"account/rateLimits/read\"}}")
                    .map_err(|_| "Cannot read Codex usage.")?;
            } else if row["id"] == 2 {
                if let Some(value) = row.get("result") {
                    return Ok(value.clone());
                }
                return Err("Usage unavailable. Check your Codex sign-in.".into());
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[tauri::command]
pub async fn get_codex_rate_limits() -> Result<Value, String> {
    // Account-wide limits only; no model turns or account mutations.
    tauri::async_runtime::spawn_blocking(read_rate_limits)
        .await
        .map_err(|_| "Cannot read Codex usage.".to_string())?
}

#[tauri::command]
pub async fn configure_codex_bridge(install: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || configure_bridge(install))
        .await
        .map_err(|_| "Cannot configure the Codex bridge.".to_string())?
}

#[tauri::command]
pub fn open_codex_chat(app: tauri::AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if !valid_id(&id) {
        return Err("Invalid Codex chat.".into());
    }
    app.opener()
        .open_url(format!("codex://threads/{id}"), None::<&str>)
        .map_err(|_| "Cannot open this chat. Check that the Codex desktop app is installed.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approval_requires_an_exact_fresh_request() {
        let mut request = CodexApproval {
            request_id: "a".repeat(32),
            session_id: "chat-1".into(),
            turn_id: "turn-1".into(),
            tool: "Bash".into(),
            command: "echo review-me".into(),
            expires_at: 121_000,
        };
        assert!(valid_request(&request, 1000));
        assert!(!valid_request(&request, 121_000));
        request.expires_at = 132_000;
        assert!(!valid_request(&request, 1000));
        request.expires_at = 2000;
        request.request_id = "../escape".into();
        assert!(!valid_request(&request, 1000));
        request.request_id = "b".repeat(32);
        request.command.clear();
        assert!(!valid_request(&request, 1000));
    }

    #[test]
    fn stale_running_is_not_presented_as_live() {
        let row = json!({"schema":1,"session_id":"chat-123","status":"running","updated_at":1000,"cwd":"C:\\repo\\bloom","detail":"Working"});
        let session = parse_session(&row, &HashMap::new(), 901_001).unwrap();
        assert_eq!(session.status, "unknown");
        assert_eq!(session.project, "bloom");
        assert!(!session.detail.contains("Approval"));
    }

    #[test]
    fn permission_attention_remains_until_a_resolution_event() {
        let row = json!({"schema":1,"session_id":"chat-123","status":"awaiting_approval","updated_at":1000,"detail":"Approval requested"});
        assert_eq!(
            parse_session(&row, &HashMap::new(), 901_001)
                .unwrap()
                .status,
            "awaiting_approval"
        );
    }

    #[test]
    fn validates_sessions_and_uses_index_title() {
        let mut row =
            json!({"schema":1,"session_id":"chat-123","status":"running","updated_at":1000});
        let titles = HashMap::from([("chat-123".into(), "Fix the island".into())]);
        assert_eq!(
            parse_session(&row, &titles, 2000).unwrap().title,
            "Fix the island"
        );
        row["session_id"] = json!("../escape");
        assert!(parse_session(&row, &titles, 2000).is_none());
        row["session_id"] = json!("chat-123");
        row["status"] = json!("closed");
        assert!(parse_session(&row, &titles, 2000).is_none());
        row["status"] = json!("running");
        assert!(parse_session(&row, &titles, 8 * 24 * 60 * 60 * 1000).is_none());
    }
}
