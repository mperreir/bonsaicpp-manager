use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use crate::locale::{lang_of, msg, Msg};

/// Handle of the running llama-server process (pid + port).
#[derive(Default)]
pub struct ServerState(
    pub Mutex<Option<ServerProc>>,
    /// Last used port (to adopt an orphaned server after restart).
    pub Mutex<Option<u16>>,
);

pub struct ServerProc {
    pub child: std::process::Child,
    pub port: u16,
    pub model: String,
    pub started_at: chrono::DateTime<chrono::Local>,
}

/// Application data folder: %APPDATA%/bonsaicpp-manager
pub fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| msg(lang_of(app), Msg::AppDataUnavailable(e.to_string())))
}

pub fn backend_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("bin"))
}

pub fn models_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("models").join("bonsai2-gguf"))
}
