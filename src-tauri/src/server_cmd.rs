//! Supervision of `llama-server`: command-line building,
//! start/stop, status and health (/health).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, State};

use crate::backend::find_installed;
use crate::locale::{lang_of, msg, Msg};
use crate::router::RouterConfig;
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

/// Router mode command line: **no `--model`**, so llama-server starts
/// with nothing loaded and each model comes in on its first API call.
///
/// Everything model-related lives in the preset (the `[*]` section holds
/// the defaults, each `[alias]` its overrides) — only the routing knobs
/// and the server knobs stay on the command line. Precedence in
/// llama.cpp: CLI > `[alias]` > `[*]`.
fn router_to_args(
    cfg: &RouterConfig,
    p: &SimplePreset,
    extra_args: &HashMap<String, String>,
    ini: &std::path::Path,
) -> Vec<String> {
    let mut a = vec![
        "--models-preset".to_string(),
        ini.display().to_string(),
        "--host".to_string(),
        p.host.clone(),
        "--port".to_string(),
        p.port.to_string(),
        "--metrics".to_string(), // required for the Stats screen
    ];
    if cfg.models_max > 0 {
        a.push("--models-max".to_string());
        a.push(cfg.models_max.to_string());
    }
    // On by default upstream; keep it explicit so the UI toggle has a
    // visible effect on the command line.
    a.push("--models-autoload".to_string());
    if cfg.sleep_idle_seconds >= 0 {
        a.push("--sleep-idle-seconds".to_string());
        a.push(cfg.sleep_idle_seconds.to_string());
    }
    for (k, v) in extra_args {
        a.push(k.clone());
        if !v.is_empty() {
            a.push(v.clone());
        }
    }
    a
}

/// Extract llama.cpp's preset rejection from the log, if it is there.
/// Emitted verbatim by the binary:
/// `E srv  llama_server: failed to initialize router models: option 'X' not recognized in preset 'Y'`
fn extract_preset_error(log_text: &str) -> Option<String> {
    const MARKER: &str = "failed to initialize router models: ";
    log_text.lines().rev().find_map(|l| {
        let after = l.split_once(MARKER).map(|(_, rest)| rest.trim())?;
        if after.is_empty() {
            None
        } else {
            Some(after.to_string())
        }
    })
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
    router: Option<RouterConfig>,
) -> Result<ServerStatus, String> {
    kill_running(&state);
    let root = backend_root(&app)?;
    let (_, _, exe) = find_installed(&root).ok_or(msg(lang_of(&app), Msg::BackendNotInstalled))?;

    // Router mode (default): nothing is loaded at launch, models arrive
    // with the first API call. Falls back to the single-model line only
    // when the registry is empty (legacy setups, first run).
    let router_mode = router.as_ref().map(|r| r.is_router()).unwrap_or(false);
    let (args, model_label) = if router_mode {
        let cfg = router.as_ref().unwrap();
        // Validate against the live --help before writing anything: an
        // unknown key makes llama.cpp exit(1), which we report precisely.
        let schema = crate::help_parser::fetch_schema(&app).await?;
        let errs = crate::router::validate(cfg, &schema);
        if let Some(first) = errs.first() {
            return Err(msg(
                lang_of(&app),
                Msg::PresetRejected(format!("{} ({:?})", first.key, first.reason)),
            ));
        }
        let ini = crate::router::write_preset(&app, cfg)?;
        (
            router_to_args(cfg, &preset, &extra_args, &ini),
            cfg.models
                .iter()
                .map(|m| m.alias.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    } else {
        if preset.model_path.is_empty() {
            return Err(msg(lang_of(&app), Msg::NoModelSelected));
        }
        let mut a = preset_to_args(&preset);
        for (k, v) in extra_args {
            a.push(k);
            if !v.is_empty() {
                a.push(v);
            }
        }
        (a, preset.model_path.clone())
    };

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
    let mut child = cmd.spawn().map_err(|e| msg(lang_of(&app), Msg::LaunchFailed(e.to_string())))?;
    let pid = child.id();
    let cmdline = format!("{} {}", exe.display(), args.join(" "));

    // A rejected preset makes llama.cpp exit immediately. Give it a beat,
    // then surface its own message instead of a phantom "running" server.
    if router_mode {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        if let Ok(Some(status)) = child.try_wait() {
            let text = std::fs::read_to_string(&log_path).unwrap_or_default();
            let detail = extract_preset_error(&text).unwrap_or_else(|| {
                format!("llama-server exited with code {}", status.code().unwrap_or(-1))
            });
            return Err(msg(lang_of(&app), Msg::PresetRejected(detail)));
        }
    }

    *state.0.lock().map_err(|e| e.to_string())? = Some(ServerProc {
        child,
        port: preset.port,
        model: model_label.clone(),
        started_at: chrono::Local::now(),
    });
    *state.1.lock().map_err(|e| e.to_string())? = Some(preset.port);
    // notify the frontend (tray/tooltip) via event
    let _ = app.emit("server-status-changed", &cmdline);
    refresh_tray_icon(&app, true);
    Ok(ServerStatus {
        running: true,
        port: Some(preset.port),
        model: Some(model_label),
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

    fn router_cfg() -> RouterConfig {
        RouterConfig {
            models: vec![crate::router::RouterModel::new("a", "a.gguf")],
            global: vec![("ctx-size".into(), "4096".into())],
            models_max: 3,
            sleep_idle_seconds: 30,
        }
    }

    fn flag_val(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    #[test]
    fn router_args_never_pass_a_model_and_carry_the_routing_knobs() {
        let p = SimplePreset::default();
        let ini = std::path::Path::new("C:/cfg/models.ini");
        let args = router_to_args(&router_cfg(), &p, &HashMap::new(), ini);

        // The whole point of router mode: no model loaded at launch.
        assert!(!args.contains(&"--model".to_string()));
        assert!(args.contains(&"--models-preset".to_string()));
        assert_eq!(
            flag_val(&args, "--models-preset").as_deref(),
            Some("C:/cfg/models.ini")
        );
        assert_eq!(flag_val(&args, "--models-max").as_deref(), Some("3"));
        assert_eq!(flag_val(&args, "--sleep-idle-seconds").as_deref(), Some("30"));
        assert!(args.contains(&"--models-autoload".to_string()));
        // Stats needs --metrics even in router mode.
        assert!(args.contains(&"--metrics".to_string()));
        assert_eq!(flag_val(&args, "--port").as_deref(), Some("8080"));
    }

    #[test]
    fn router_args_omit_models_max_when_unlimited() {
        let mut cfg = router_cfg();
        cfg.models_max = 0;
        let args = router_to_args(&cfg, &SimplePreset::default(), &HashMap::new(), std::path::Path::new("m.ini"));
        assert!(!args.contains(&"--models-max".to_string()));
    }

    #[test]
    fn router_args_omit_sleep_when_disabled() {
        let mut cfg = router_cfg();
        cfg.sleep_idle_seconds = -1;
        let args = router_to_args(&cfg, &SimplePreset::default(), &HashMap::new(), std::path::Path::new("m.ini"));
        assert!(!args.contains(&"--sleep-idle-seconds".to_string()));
    }

    #[test]
    fn router_args_append_extra_server_args() {
        let mut extra = HashMap::new();
        extra.insert("--api-key".to_string(), "k".to_string());
        extra.insert("--jinja".to_string(), String::new());
        let args = router_to_args(&router_cfg(), &SimplePreset::default(), &extra, std::path::Path::new("m.ini"));
        assert_eq!(flag_val(&args, "--api-key").as_deref(), Some("k"));
        assert!(args.contains(&"--jinja".to_string()));
    }

    #[test]
    fn preset_error_is_extracted_from_the_server_log() {
        let log = "0.00.043.401 I srv   load_models: Loaded 0 cached model presets\n0.00.046.667 E srv  llama_server: failed to initialize router models: option 'bogus' not recognized in preset 'a'\n";
        assert_eq!(
            extract_preset_error(log).as_deref(),
            Some("option 'bogus' not recognized in preset 'a'")
        );
    }

    #[test]
    fn preset_error_absent_in_a_healthy_log() {
        let log = "0.00.043.401 I srv   load_models: Loaded 2 custom model presets from C:/x.ini\n[49574] I srv model loaded\n";
        assert!(extract_preset_error(log).is_none());
    }

    #[test]
    fn preset_error_picks_the_last_rejection() {
        let log = "E srv failed to initialize router models: option 'first' not recognized in preset 'a'\nnoise\nE srv failed to initialize router models: option 'second' not recognized in preset 'b'\n";
        assert_eq!(
            extract_preset_error(log).as_deref(),
            Some("option 'second' not recognized in preset 'b'")
        );
    }
}
