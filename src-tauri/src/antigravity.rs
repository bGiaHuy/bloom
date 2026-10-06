//! Antigravity IDE and Antigravity 2.0 lifecycle tracking, Notch integration and prompt dispatch.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravitySession {
    pub id: String,
    pub title: String,
    pub project: String,
    pub status: String, // "idle" | "running" | "awaiting_approval" | "completed" | "interrupted"
    pub detail: String,
    pub updated_at: u64,
    pub source: String, // "IDE" | "Antigravity 2.0"
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravitySnapshot {
    pub installed: bool,
    pub ide_installed: bool,
    pub app_installed: bool,
    pub sessions: Vec<AntigravitySession>,
}

fn gemini_root() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".gemini"))
}

fn is_antigravity_installed() -> (bool, bool) {
    let ide = std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("Programs").join("Antigravity IDE"))
        .is_some_and(|p| p.is_dir());
    let app = std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("agy").join("bin").join("agy.exe"))
        .is_some_and(|p| p.is_file());
    (ide, app)
}

fn load_known_projects() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(root) = gemini_root() else { return map; };
    let projects_file = root.join("projects.json");
    if let Ok(bytes) = std::fs::read(projects_file) {
        if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(obj) = val["projects"].as_object() {
                for (path, name) in obj {
                    if let Some(name_str) = name.as_str() {
                        map.insert(name_str.to_lowercase(), path.clone());
                    }
                }
            }
        }
    }
    map
}

fn clipped(value: &str, length: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(length)
        .collect()
}

fn parse_transcript_file(
    path: &Path,
    source_name: &str,
    now: u64,
    known_projects: &HashMap<String, String>,
) -> Option<AntigravitySession> {
    let Ok(mut file) = File::open(path) else {
        return None;
    };
    let Ok(metadata) = file.metadata() else {
        return None;
    };
    let len = metadata.len();
    if len == 0 {
        return None;
    }

    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(now);

    let age = now.saturating_sub(mtime);

    // Read head for title & project (up to 8KB)
    let head_size = (len.min(8192)) as usize;
    let mut head_buf = vec![0u8; head_size];
    if file.read_exact(&mut head_buf).is_err() {
        return None;
    }
    let head_str = String::from_utf8_lossy(&head_buf);

    // Extract title from first USER_INPUT
    let mut title = String::new();
    for line in head_str.lines() {
        if line.contains("\"type\":\"USER_INPUT\"") || line.contains("USER_INPUT") {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                if let Some(content) = val["content"].as_str() {
                    let mut text = content.trim();
                    if let Some(start) = text.find("<USER_REQUEST>") {
                        let rem = &text[start + "<USER_REQUEST>".len()..];
                        if let Some(end) = rem.find("</USER_REQUEST>") {
                            text = &rem[..end];
                        } else {
                            text = rem;
                        }
                    }
                    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    title = clipped(&cleaned, 120);
                    break;
                }
            }
        }
    }

    // Extract project and workspace path from head
    let mut project = String::new();
    let mut workspace_path = None;

    for part in head_str.split(&['\\', '/'][..]) {
        if let Some(rest) = part.strip_prefix("Codespace") {
            let p = rest.trim_start_matches(['\\', '/']).trim();
            if !p.is_empty() && p.len() < 50 {
                project = clipped(p, 40);
                break;
            }
        }
    }

    if !project.is_empty() {
        if let Some(known_path) = known_projects.get(&project.to_lowercase()) {
            workspace_path = Some(known_path.clone());
        }
    }

    // If not found in known projects, try searching for full path pattern
    if workspace_path.is_none() {
        for line in head_str.lines() {
            if let Some(idx) = line.find("Codespace") {
                let prefix_start = line[..idx].rfind(|c: char| c == '"' || c == '\'' || c == ' ' || c == '<' || c == '(').map(|i| i + 1).unwrap_or(0);
                let suffix_end = line[idx..].find(|c: char| c == '"' || c == '\'' || c == '\n' || c == '\r' || c == '>' || c == ')').map(|i| idx + i).unwrap_or(line.len());
                let candidate = line[prefix_start..suffix_end].replace("\\\\", "\\").trim().to_string();
                if candidate.contains(':') && Path::new(&candidate).is_dir() {
                    workspace_path = Some(candidate);
                    break;
                }
            }
        }
    }

    // Read tail for last step (up to 16KB)
    let tail_start = len.saturating_sub(16384);
    if file.seek(SeekFrom::Start(tail_start)).is_err() {
        return None;
    }
    let mut tail_buf = Vec::new();
    if file.read_to_end(&mut tail_buf).is_err() {
        return None;
    }
    let tail_str = String::from_utf8_lossy(&tail_buf);

    let mut last_line = "";
    for line in tail_str.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            last_line = trimmed;
            break;
        }
    }

    let mut status = "idle".to_string();
    let mut detail = String::new();

    if let Ok(last_obj) = serde_json::from_str::<Value>(last_line) {
        let stype = last_obj["type"].as_str().unwrap_or("");
        let tool_calls = last_obj["tool_calls"].as_array();

        let mut has_ask_question = false;
        let mut question_summary = String::new();

        if let Some(calls) = tool_calls {
            for call in calls {
                if call["name"].as_str() == Some("ask_question") {
                    has_ask_question = true;
                    if let Some(args) = call["args"].as_object() {
                        if let Some(s) = args.get("toolSummary").and_then(|v| v.as_str()) {
                            question_summary = s.trim_matches('"').to_string();
                        } else if let Some(s) = args.get("toolAction").and_then(|v| v.as_str()) {
                            question_summary = s.trim_matches('"').to_string();
                        }
                    }
                    break;
                }
            }
        }

        if has_ask_question {
            status = "awaiting_approval".to_string();
            detail = if question_summary.is_empty() {
                "Waiting for your response in Antigravity".to_string()
            } else {
                question_summary
            };
        } else if age < 45_000 {
            if stype == "USER_INPUT" {
                status = "running".to_string();
                detail = "Thinking / Processing prompt...".to_string();
            } else if let Some(calls) = tool_calls {
                if let Some(first_call) = calls.first() {
                    status = "running".to_string();
                    let cname = first_call["name"].as_str().unwrap_or("tool");
                    let mut tool_desc = String::new();
                    if let Some(args) = first_call["args"].as_object() {
                        if let Some(s) = args.get("toolSummary").and_then(|v| v.as_str()) {
                            tool_desc = s.trim_matches('"').to_string();
                        } else if let Some(s) = args.get("toolAction").and_then(|v| v.as_str()) {
                            tool_desc = s.trim_matches('"').to_string();
                        }
                    }
                    if tool_desc.is_empty() {
                        detail = format!("Running {cname}");
                    } else {
                        detail = tool_desc;
                    }
                } else {
                    status = "completed".to_string();
                    detail = "Task completed".to_string();
                }
            } else {
                status = "completed".to_string();
                detail = "Finished".to_string();
            }
        } else {
            status = "completed".to_string();
            detail = "Finished".to_string();
        }
    }

    // Parent folder 3 levels up is conversation_id: brain/<cid>/.system_generated/logs/transcript.jsonl
    let id = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    if title.is_empty() {
        title = format!("{source_name} · {}", &id[..id.len().min(8)]);
    }
    if project.is_empty() {
        project = if source_name == "IDE" {
            "Antigravity".to_string()
        } else {
            "Antigravity 2.0".to_string()
        };
    }

    Some(AntigravitySession {
        id,
        title,
        project,
        status,
        detail,
        updated_at: mtime,
        source: source_name.to_string(),
        workspace_path,
    })
}

fn read_snapshot() -> Result<AntigravitySnapshot, String> {
    let (ide_installed, app_installed) = is_antigravity_installed();
    let installed = ide_installed || app_installed;

    let Some(root) = gemini_root() else {
        return Ok(AntigravitySnapshot {
            installed,
            ide_installed,
            app_installed,
            sessions: Vec::new(),
        });
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let known_projects = load_known_projects();

    let sources = [
        ("IDE", root.join("antigravity-ide").join("brain")),
        ("Antigravity 2.0", root.join("antigravity").join("brain")),
    ];

    let mut candidate_files: Vec<(u64, PathBuf, &'static str)> = Vec::new();

    for (source_name, brain_dir) in sources {
        let Ok(entries) = std::fs::read_dir(&brain_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let t_path = entry
                .path()
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl");
            if let Ok(meta) = t_path.metadata() {
                if meta.is_file() {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    candidate_files.push((mtime, t_path, source_name));
                }
            }
        }
    }

    candidate_files.sort_by(|a, b| b.0.cmp(&a.0));
    candidate_files.truncate(24);

    let mut sessions = Vec::new();
    for (_, path, source_name) in candidate_files {
        if let Some(session) = parse_transcript_file(&path, source_name, now, &known_projects) {
            sessions.push(session);
        }
    }

    // Sort: awaiting_approval first, then running, then most recent
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

    Ok(AntigravitySnapshot {
        installed,
        ide_installed,
        app_installed,
        sessions,
    })
}

#[tauri::command]
pub async fn get_antigravity_sessions() -> Result<AntigravitySnapshot, String> {
    tauri::async_runtime::spawn_blocking(read_snapshot)
        .await
        .map_err(|e| format!("Cannot read Antigravity status: {e}"))?
}

fn focus_antigravity_window() -> bool {
    unsafe {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SetForegroundWindow, ShowWindow, SW_RESTORE,
        };

        struct EnumData {
            found_hwnd: Option<HWND>,
        }

        unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let data = &mut *(lparam.0 as *mut EnumData);
            if !IsWindowVisible(hwnd).as_bool() {
                return windows::core::BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return windows::core::BOOL(1);
            }

            if let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut buf = [0u16; 260];
                let mut size = buf.len() as u32;
                let ok = QueryFullProcessImageNameW(
                    proc,
                    PROCESS_NAME_FORMAT(0),
                    windows::core::PWSTR(buf.as_mut_ptr()),
                    &mut size,
                );
                let _ = windows::Win32::Foundation::CloseHandle(proc);
                if ok.is_ok() {
                    let path = String::from_utf16_lossy(&buf[..size as usize]).to_lowercase();
                    if path.contains("antigravity") || path.contains("agy") {
                        data.found_hwnd = Some(hwnd);
                        return windows::core::BOOL(0);
                    }
                }
            }
            windows::core::BOOL(1)
        }

        let mut data = EnumData { found_hwnd: None };
        let callback: unsafe extern "system" fn(HWND, LPARAM) -> windows::core::BOOL = enum_callback;
        let _ = EnumWindows(Some(callback), LPARAM(&mut data as *mut EnumData as isize));

        if let Some(hwnd) = data.found_hwnd {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
            true
        } else {
            false
        }
    }
}

#[tauri::command]
pub fn open_antigravity_chat(app: tauri::AppHandle, _id: String) -> Result<(), String> {
    if focus_antigravity_window() {
        return Ok(());
    }

    // Launch Antigravity IDE if not running
    if let Some(local_app) = std::env::var_os("LOCALAPPDATA") {
        let exe = PathBuf::from(local_app)
            .join("Programs")
            .join("Antigravity IDE")
            .join("Antigravity IDE.exe");
        if exe.is_file() {
            let _ = std::process::Command::new(exe).spawn();
            return Ok(());
        }
    }

    Err("Could not find or launch Antigravity.".into())
}

#[tauri::command]
pub fn focus_antigravity() -> Result<(), String> {
    if focus_antigravity_window() {
        Ok(())
    } else {
        Err("Antigravity window not found.".into())
    }
}

#[tauri::command]
pub async fn send_antigravity_prompt(
    prompt: String,
    session_id: Option<String>,
    workspace_path: Option<String>,
    source: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let clean_prompt = prompt.trim();
        if clean_prompt.is_empty() {
            return Err("Prompt cannot be empty.".into());
        }

        let cwd = workspace_path
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));

        let is_app_source = source
            .as_deref()
            .map(|s| s.contains("2.0") || s.contains("App"))
            .unwrap_or(false);

        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        if is_app_source {
            let agy_path = std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("agy").join("bin").join("agy.exe"))
                .filter(|p| p.is_file());

            if let Some(agy) = agy_path {
                let mut cmd = std::process::Command::new(agy);
                if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
                    cmd.args(["--conversation", &sid, "-p", clean_prompt]);
                } else {
                    cmd.args(["-p", clean_prompt]);
                }
                cmd.current_dir(&cwd);
                cmd.creation_flags(CREATE_NO_WINDOW);
                cmd.spawn()
                    .map_err(|e| format!("Failed to launch agy: {e}"))?;
                return Ok("Prompt sent to Antigravity 2.0".into());
            }
        }

        let ide_cmd = std::env::var_os("LOCALAPPDATA")
            .map(|p| {
                PathBuf::from(p)
                    .join("Programs")
                    .join("Antigravity IDE")
                    .join("bin")
                    .join("antigravity-ide.cmd")
            })
            .filter(|p| p.is_file());

        if let Some(cmd_path) = ide_cmd {
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.args(["/C", &cmd_path.to_string_lossy(), "chat", "-r", clean_prompt]);
            cmd.current_dir(&cwd);
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd.spawn()
                .map_err(|e| format!("Failed to send chat to Antigravity IDE: {e}"))?;

            focus_antigravity_window();
            return Ok("Prompt sent to Antigravity IDE".into());
        }

        Err("Antigravity executable not found.".into())
    })
    .await
    .map_err(|e| format!("Task failed: {e}"))?
}
