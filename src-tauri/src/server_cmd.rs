//! Supervision of `llama-server`: command-line building,
//! start/stop, status and health (/health).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, State};

use crate::backend::find_installed;
use crate::locale::{lang_of, msg, Msg};
use crate::state::{backend_root, ServerProc, ServerState};
use crate::tray_icon::refresh_tray_icon;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub model: Option<String>,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub cmdline: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimplePreset {
    pub model_path: String,
    pub ctx: u32,
    pub ngl: i32,
    pub threads: u32,
    pub host: String,
    pub port: u16,
    pub reasoning_effort: String,
    pub mmproj_path: Option<String>,
}

impl Default for SimplePreset {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            ctx: 0, // 0 = auto RAM-tiered size (Bonsai-demo scripts)
            ngl: -1, // -1 = GPU auto-detect
            threads: 0, // 0 = auto
            host: "127.0.0.1".to_string(),
            port: 8080,
            reasoning_effort: "xhigh".to_string(),
            mmproj_path: None,
        }
    }
}

fn preset_to_args(p: &SimplePreset) -> Vec<String> {
    let mut a = vec![
        "--model".to_string(),
        p.model_path.clone(),
        "--host".to_string(),
        p.host.clone(),
        "--port".to_string(),
        p.port.to_string(),
        "--metrics".to_string(), // required for the Stats screen
        "--chat-template-kwargs".to_string(),
        format!("{{\"reasoning_effort\":\"{}\"}}", p.reasoning_effort),
    ];
    if p.ctx > 0 {
        a.push("--ctx-size".to_string());
        a.push(p.ctx.to_string());
    }
    if p.ngl >= 0 {
        a.push("--n-gpu-layers".to_string());
        a.push(p.ngl.to_string());
    }
    if p.threads > 0 {
        a.push("--threads".to_string());
        a.push(p.threads.to_string());
    }
    if let Some(mm) = &p.mmproj_path {
        if !mm.is_empty() {
            a.push("--mmproj".to_string());
            a.push(mm.clone());
        }
    }
    a
}

#[tauri::command]
pub async fn build_cmdline(
    preset: SimplePreset,
    extra_args: HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut args = preset_to_args(&preset);
    for (k, v) in extra_args {
        args.push(k);
        if !v.is_empty() {
            args.push(v);
        }
    }
    Ok(args)
}

fn kill_running(state: &ServerState) {
    if let Ok(mut guard) = state.0.lock() {
        if let Some(mut proc) = guard.take() {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }
}

/// True if a server is running: tracked child alive, or an orphan
/// answering on the last known port (same sources as `server_status`).
pub(crate) async fn is_server_running(state: &ServerState) -> bool {
    {
        let mut guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if let Some(proc) = guard.as_mut() {
            match proc.child.try_wait() {
                Ok(None) => return true,
                Ok(Some(_)) => {
                    guard.take();
                }
                Err(_) => {}
            }
        }
    }
    let port = state.1.lock().ok().and_then(|g| *g).or(Some(8080));
    match port {
        Some(p) => probe_health(p).await,
        None => false,
    }
}

/// Quick /health check: is the port being served?
async fn probe_health(port: u16) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
    {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

/// `true` for a LISTENING state in any Windows display language
/// (EN `LISTENING`, FR `ÉCOUTANT`/`ECOUTE`, DE `ABHÖREN`, ES `ESCUCHA`…).
/// Matching is intentionally loose: netstat on a French Windows speaks
/// French, and OEM codepage output may mojibake accents.
fn is_listening_state(state: &str) -> bool {
    let u = state.to_uppercase();
    u.contains("LISTEN") || u.contains("COUT") || u.contains("COUTE") || u.contains("HREN") || u.contains("HOR") || u.contains("ESCUCH")
}

/// PID listening on the port (Windows, via netstat): pure parser, tested.
/// First pass: listening sockets only; second pass: any TCP line whose local
/// address matches (locale-proof fallback if the state word is unrecognized).
fn parse_netstat_pid(text: &str, port: u16) -> Option<u32> {
    let suffix = format!(":{port}");
    let mut fallback = None;
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 && parts[0] == "TCP" && parts[1].ends_with(suffix.as_str()) {
            if let Ok(pid) = parts[4].parse::<u32>() {
                if pid == 0 {
                    continue;
                }
                if parts[3] == "LISTENING" || is_listening_state(parts[3]) {
                    return Some(pid);
                }
                fallback.get_or_insert(pid);
            }
        }
    }
    fallback
}

/// PID listening on the port (Windows, via netstat).
fn pid_listening_on(port: u16) -> Option<u32> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("netstat");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let out = cmd.args(["-ano"]).output().ok()?;
    parse_netstat_pid(&String::from_utf8_lossy(&out.stdout), port)
}

/// Executable name of a PID (safeguard before taskkill).
fn process_name(pid: u32) -> Option<String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("tasklist");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let out = cmd
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .to_string();
    // "llama-server.exe","6108","Console",...
    let name = line.split(',').next()?.trim().trim_matches('"').to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[tauri::command]
pub async fn start_server(
    app: AppHandle,
    state: State<'_, ServerState>,
    preset: SimplePreset,
    extra_args: HashMap<String, String>,
) -> Result<ServerStatus, String> {
    kill_running(&state);
    let root = backend_root(&app)?;
    let (_, _, exe) = find_installed(&root).ok_or(msg(lang_of(&app), Msg::BackendNotInstalled))?;
    if preset.model_path.is_empty() {
        return Err(msg(lang_of(&app), Msg::NoModelSelected));
    }
    let args = build_cmdline(preset.clone(), extra_args).await?;
    let log_dir = crate::state::app_data_dir(&app)?.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let log_path = log_dir.join("llama-server.log");
    let log_file = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;

    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args)
        .stdout(log_file.try_clone().map_err(|e| e.to_string())?)
        .stderr(log_file)
        .current_dir(exe.parent().unwrap());
    #[cfg(windows)]
    {
        // DETACHED_PROCESS: survive the window + no console.
        cmd.creation_flags(0x00000008 | 0x08000000);
    }
    let child = cmd.spawn().map_err(|e| msg(lang_of(&app), Msg::LaunchFailed(e.to_string())))?;
    let pid = child.id();
    let cmdline = format!("{} {}", exe.display(), args.join(" "));
    *state.0.lock().map_err(|e| e.to_string())? = Some(ServerProc {
        child,
        port: preset.port,
        model: preset.model_path.clone(),
        started_at: chrono::Local::now(),
    });
    *state.1.lock().map_err(|e| e.to_string())? = Some(preset.port);
    // notify the frontend (tray/tooltip) via event
    let _ = app.emit("server-status-changed", &cmdline);
    refresh_tray_icon(&app, true);
    Ok(ServerStatus {
        running: true,
        port: Some(preset.port),
        model: Some(preset.model_path),
        pid: Some(pid),
        uptime_secs: Some(0),
        cmdline: Some(cmdline),
    })
}

#[tauri::command]
pub async fn stop_server(
    app: AppHandle,
    state: State<'_, ServerState>,
) -> Result<ServerStatus, String> {
    stop_server_impl(&app, &state).await
}

/// Shared stop logic (also used before a backend update/install).
pub(crate) async fn stop_server_impl(
    app: &AppHandle,
    state: &ServerState,
) -> Result<ServerStatus, String> {
    kill_running(&state);
    // Cleanup of a possible orphan (e.g. app relaunched): only kill if
    // the port answers AND the process really is llama-server.exe.
    let port = state.1.lock().ok().and_then(|g| *g).or(Some(8080));
    if let Some(p) = port {
        if probe_health(p).await {
            if let Some(pid) = pid_listening_on(p) {
                let ours = process_name(pid)
                    .map(|n| n.eq_ignore_ascii_case("llama-server.exe"))
                    .unwrap_or(false);
                if ours {
                    #[cfg(windows)]
                    use std::os::windows::process::CommandExt;
                    let mut kill = std::process::Command::new("taskkill");
                    #[cfg(windows)]
                    kill.creation_flags(0x08000000);
                    let _ = kill
                        .args(["/PID", &pid.to_string(), "/F"])
                        .output();
                    for _ in 0..10 {
                        if !probe_health(p).await {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }
    }
    let _ = app.emit("server-status-changed", "stopped");
    refresh_tray_icon(app, false);
    Ok(ServerStatus {
        running: false,
        port: None,
        model: None,
        pid: None,
        uptime_secs: None,
        cmdline: None,
    })
}

#[tauri::command]
pub async fn server_status(app: AppHandle, state: State<'_, ServerState>) -> Result<ServerStatus, String> {
    {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(proc) = guard.as_mut() {
            match proc.child.try_wait().map_err(|e| e.to_string())? {
                Some(_) => {
                    // process ended on its own (crash / bad args)
                    let p = guard.take().unwrap();
                    let uptime =
                        (chrono::Local::now() - p.started_at).num_seconds().max(0) as u64;
                    let _ = uptime;
                    refresh_tray_icon(&app, false);
                    return Ok(ServerStatus {
                        running: false,
                        port: None,
                        model: None,
                        pid: None,
                        uptime_secs: None,
                        cmdline: None,
                    });
                }
                None => {
                    let uptime = (chrono::Local::now() - proc.started_at)
                        .num_seconds()
                        .max(0) as u64;
                    refresh_tray_icon(&app, true);
                    return Ok(ServerStatus {
                        running: true,
                        port: Some(proc.port),
                        model: Some(proc.model.clone()),
                        pid: Some(proc.child.id()),
                        uptime_secs: Some(uptime),
                        cmdline: None,
                    });
                }
            }
        }
    }
    // No tracked child (e.g. app relaunched): adopt the server if the
    // last known port answers. Without this, the app believes it is stopped
    // and stats polling stays off while the server is running.
    let port = state
        .1
        .lock()
        .ok()
        .and_then(|g| *g)
        .or(Some(8080));
    if let Some(p) = port {
        if probe_health(p).await {
            refresh_tray_icon(&app, true);
            return Ok(ServerStatus {
                running: true,
                port: Some(p),
                model: None,
                pid: None,
                uptime_secs: None,
                cmdline: None,
            });
        }
    }
    refresh_tray_icon(&app, false);
    Ok(ServerStatus {
        running: false,
        port: None,
        model: None,
        pid: None,
        uptime_secs: None,
        cmdline: None,
    })
}

/// Health via GET /health (complements server_status for the UI, not a replacement).
#[tauri::command]
pub async fn server_health(port: u16) -> Result<serde_json::Value, String> {
    let url = format!("http://127.0.0.1:{}/health", port);
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("/health: {e}"))?
        .json()
        .await
        .map_err(|e| format!("JSON /health: {e}"))
}

/// Return the last 200 lines of the server log.
#[tauri::command]
pub async fn server_log_tail(app: AppHandle, lines: Option<usize>) -> Result<String, String> {
    let n = lines.unwrap_or(200).min(2000);
    let path = crate::state::app_data_dir(&app)?.join("logs").join("llama-server.log");
    if !path.exists() {
        return Ok(String::new());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(n);
    Ok(all[start..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netstat_parses_english_listening() {
        let out = "  TCP    127.0.0.1:8080         0.0.0.0:0              LISTENING       6108\n";
        assert_eq!(parse_netstat_pid(out, 8080), Some(6108));
        assert_eq!(parse_netstat_pid(out, 8081), None);
    }

    #[test]
    fn netstat_parses_french_ecoutant() {
        // Windows FR: `netstat -ano` prints the state in French.
        let out = "  TCP    127.0.0.1:8080         0.0.0.0:0              \u{c9}COUTANT       6108\n";
        assert_eq!(parse_netstat_pid(out, 8080), Some(6108));
    }

    #[test]
    fn netstat_ignores_pid_zero_and_wrong_port() {
        let out = "  TCP    0.0.0.0:8080           0.0.0.0:0              LISTENING       0\n  TCP    127.0.0.1:9090         0.0.0.0:0              LISTENING       42\n";
        assert_eq!(parse_netstat_pid(out, 8080), None);
        assert_eq!(parse_netstat_pid(out, 9090), Some(42));
    }
}
