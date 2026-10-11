mod backend;
mod gpu;
mod help_parser;
mod locale;
mod models;
mod server_cmd;
mod state;
mod stats;
mod tray_icon;

use locale::{lang_of, start_lang, tray_labels, AppLang, Lang};
use state::ServerState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};

/// Alias to avoid confusion with the `stats` module.
type StatsStateAlias = stats::StatsState;

/// Launch at user login (Windows registry via the plugin).
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable/disable launch at login. Returns the actual state.
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, on: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let a = app.autolaunch();
    if on {
        a.enable()
    } else {
        a.disable()
    }
    .map_err(|e| e.to_string())?;
    a.is_enabled().map_err(|e| e.to_string())
}

/// Version + build date for the About page. The build date is the
/// executable's link time (no build script needed).
#[derive(serde::Serialize)]
struct AboutInfo {
    version: String,
    built_at: String,
}

#[tauri::command]
fn about_info(app: tauri::AppHandle) -> AboutInfo {
    about_info_inner(lang_of(&app))
}

fn about_info_inner(lang: Lang) -> AboutInfo {
    let built_at = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(&p).ok())
        .and_then(|m| m.modified().ok())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| locale::msg(lang, locale::Msg::BuildUnknown));
    AboutInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        built_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_info_reports_version() {
        let info = about_info_inner(Lang::Fr);
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.built_at.is_empty());
    }
}

/// Tray menu items, kept to relabel them on language switch
/// (`set_app_language`) without rebuilding the tray.
#[derive(Clone)]
pub struct TrayMenu {
    show: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
    open_ui: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

impl TrayMenu {
    pub(crate) fn apply_labels(&self, lang: Lang) -> tauri::Result<()> {
        let [show, toggle, open_ui, quit] = tray_labels(lang);
        self.show.set_text(show)?;
        self.toggle.set_text(toggle)?;
        self.open_ui.set_text(open_ui)?;
        self.quit.set_text(quit)?;
        Ok(())
    }
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let [show_t, toggle_t, open_ui_t, quit_t] = tray_labels(lang_of(app));
    let show = MenuItem::with_id(app, "show", show_t, true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", toggle_t, true, None::<&str>)?;
    let open_ui = MenuItem::with_id(app, "open_ui", open_ui_t, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit_t, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &open_ui, &quit])?;
    app.manage(TrayMenu { show: show.clone(), toggle: toggle.clone(), open_ui: open_ui.clone(), quit: quit.clone() });

    let _tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Bonsai.cpp Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "toggle" => {
                // Stop a running server (tracked child or adopted orphan).
                // Starting needs the user's model preset (frontend-owned
                // localStorage), so when stopped we show the window instead
                // of guessing a command line.
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let st = handle.state::<ServerState>();
                    if server_cmd::is_server_running(&st).await {
                        let _ = server_cmd::stop_server_impl(&handle, &st).await;
                    } else if let Some(w) = handle.get_webview_window("main") {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                });
            }
            "open_ui" => {
                // Last known port, not hardcoded :8080 (the user may remap it).
                let port = app
                    .state::<ServerState>()
                    .1
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .unwrap_or(8080);
                let _ = tauri_plugin_opener::open_url(
                    format!("http://localhost:{port}"),
                    None::<&str>,
                );
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::TrayIconEvent;
            // Single left click: show the window.
            if let TrayIconEvent::Click { button, button_state, .. } = event {
                use tauri::tray::MouseButton::Left;
                use tauri::tray::MouseButtonState::Up;
                if button == Left && button_state == Up {
                    let app = tray.app_handle();
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ServerState::default())
        .manage(StatsStateAlias::default())
        .manage(AppLang::default())
        .manage(tray_icon::TrayIconState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            // Resolve the UI language before building the tray so the
            // menu speaks it on the first paint (no French flash).
            let lang = start_lang(app.handle());
            *app.state::<AppLang>().0.lock().unwrap_or_else(|e| e.into_inner()) = lang;
            build_tray(app.handle())?;
            // No server runs at launch (backend memory is fresh): gray icon.
            tray_icon::refresh_tray_icon(app.handle(), false);
            // --minimized (autostart): start hidden to the tray.
            if std::env::args().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Close = hide to the tray, the server keeps running in the background.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            backend::detect_gpu,
            backend::backend_status,
            backend::list_backend_releases,
            backend::install_backend,
            backend::list_installed_backends,
            backend::set_default_backend,
            backend::delete_backend,
            help_parser::parse_server_help,
            models::list_local_models,
            models::list_remote_files,
            models::download_model_file,
            models::delete_model_file,
            server_cmd::build_cmdline,
            server_cmd::start_server,
            server_cmd::stop_server,
            server_cmd::server_status,
            server_cmd::server_health,
            server_cmd::server_log_tail,
            stats::scrape_metrics,
            stats::poll_activity,
            stats::send_test_prompt,
            stats::record_timing,
            stats::set_tray_tooltip,
            gpu::gpu_memory,
            about_info,
            get_autostart,
            set_autostart,
            locale::set_app_language,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
