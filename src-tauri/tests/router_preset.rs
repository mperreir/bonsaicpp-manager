//! End-to-end check of the generated `--models-preset` INI against the real
//! `llama-server` binary.
//!
//! The one test that cannot be faked with a snapshot: llama-server **exits
//! 1** on a preset key it does not know
//! (`option 'X' not recognized in preset 'Y'`), so the only authority on
//! whether `RouterConfig::render_ini` emits a usable file is the binary
//! itself. It also confirms the two-level `[*]` / `[alias]` layout and that
//! `load-on-startup` is honoured.
//!
//! Ignored by default: it needs a backend installed under
//! `%APPDATA%\com.bonsai.manager\bin` and a downloaded model.
//! ```powershell
//! cargo test --test router_preset -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use bonsaicpp_manager_lib::router::{RouterConfig, RouterModel};

const PORT: u16 = 8123;

fn server_exe() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("APPDATA").ok()?).join("com.bonsai.manager").join("bin");
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .flat_map(|tag| {
            std::fs::read_dir(tag.path())
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|f| f.ok())
                .map(|f| f.path().join("llama-server.exe"))
                .filter(|p| p.exists())
                .collect::<Vec<_>>()
        })
        .collect();
    found.sort();
    found.pop()
}

fn gguf_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            out.extend(gguf_files(&p));
        } else if p.extension().and_then(|s| s.to_str()) == Some("gguf") {
            out.push(p);
        }
    }
    out
}

/// Smallest downloaded model: fastest to load.
fn local_model() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("APPDATA").ok()?)
        .join("com.bonsai.manager")
        .join("models");
    let mut found = gguf_files(&dir);
    found.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX));
    found.into_iter().next()
}

fn curl(url: &str) -> String {
    std::process::Command::new("curl")
        .args(["-s", url])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn http_code(url: &str) -> u16 {
    std::process::Command::new("curl")
        .args(["-s", "-o", "NUL", "-w", "%{http_code}", url])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

fn wait_health(port: u16, timeout_secs: u32) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs as u64);
    while std::time::Instant::now() < deadline {
        let c = http_code(&format!("http://127.0.0.1:{port}/health"));
        if c == 200 || c == 503 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

#[test]
#[ignore = "needs an installed backend and a downloaded model"]
fn generated_ini_is_accepted_by_llama_server() {
    let Some(exe) = server_exe() else {
        panic!("no llama-server.exe under %APPDATA%\\com.bonsai.manager\\bin");
    };
    let Some(model) = local_model() else {
        panic!("no .gguf under %APPDATA%\\com.bonsai.manager\\models");
    };

    // The real generator, not a hand-written fixture.
    let mut cfg = RouterConfig {
        models_max: 2,
        sleep_idle_seconds: -1,
        ..Default::default()
    };
    cfg.global = vec![
        ("ctx-size".into(), "2048".into()),
        ("flash-attn".into(), "true".into()),
        ("n-gpu-layers".into(), "0".into()),
    ];
    let mut small = RouterModel::new("small", model.to_string_lossy().to_string());
    small.routed = true;
    small.overrides = vec![("ctx-size".into(), "1024".into())];
    let mut pre = RouterModel::new("preloaded", model.to_string_lossy().to_string());
    pre.routed = true;
    pre.load_on_startup = true;
    cfg.models = vec![small, pre];

    let stem = format!("bonsai-e2e-{}", std::process::id());
    let ini = std::env::temp_dir().join(format!("{stem}.ini"));
    let log = std::env::temp_dir().join(format!("{stem}.log"));
    std::fs::write(&ini, cfg.render_ini()).expect("write ini");

    let logf = std::fs::File::create(&log).expect("create log");
    let mut child = std::process::Command::new(&exe)
        .args([
            "--models-preset",
            ini.to_str().unwrap(),
            "--models-max",
            "2",
            "--metrics",
            "--host",
            "127.0.0.1",
            "--port",
            &PORT.to_string(),
        ])
        .stdout(logf.try_clone().expect("clone log"))
        .stderr(logf)
        .spawn()
        .expect("spawn llama-server");

    let up = wait_health(PORT, 120);
    let registry = if up {
        curl(&format!("http://127.0.0.1:{PORT}/models"))
    } else {
        String::new()
    };

    let _ = child.kill();
    let _ = child.wait();
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();

    assert!(
        up,
        "server never answered /health.\n--- preset ---\n{}\n--- log ---\n{}",
        cfg.render_ini(),
        log_text
    );
    assert!(
        registry.contains("\"small\""),
        "[small] missing from the registry: {registry}"
    );
    assert!(
        registry.contains("\"preloaded\""),
        "[preloaded] missing from the registry: {registry}"
    );
    assert!(
        !log_text.contains("not recognized in preset"),
        "llama-server refused a generated key:\n{log_text}"
    );
    assert!(
        !log_text.contains("failed to initialize router models"),
        "router initialisation failed:\n{log_text}"
    );
    // Direct proof that *our* file was read, and that both sections were
    // registered (llama-server logs one line with the count).
    assert!(
        log_text.contains("2 custom model presets"),
        "llama-server did not report 2 presets from the generated file:\n{log_text}"
    );
    // `load-on-startup = true` must actually start loading that model.
    assert!(
        registry.contains("loading") || registry.contains("loaded"),
        "preloaded section did not start loading:\n{registry}"
    );

    // An un-routed model must stay out of the preset entirely.
    let mut cfg2 = cfg.clone();
    cfg2.models[0].routed = false;
    let ini2 = std::env::temp_dir().join(format!("{stem}-unrouted.ini"));
    std::fs::write(&ini2, cfg2.render_ini()).expect("write ini 2");
    assert!(
        !cfg2.render_ini().contains("[small]"),
        "an un-routed model leaked into the preset:\n{}",
        cfg2.render_ini()
    );
    let _ = std::fs::remove_file(&ini2);

    let _ = std::fs::remove_file(&ini);
    let _ = std::fs::remove_file(&log);
}
