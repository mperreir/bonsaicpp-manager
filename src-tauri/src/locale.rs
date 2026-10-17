//! UI language (French/English): tray menu labels + user-facing errors.
//!
//! The frontend owns the choice (persisted `bonsai.locale`); it pushes it
//! here via `set_app_language` so the tray menu follows. At startup the
//! backend reuses its own persisted file, else the OS locale
//! (`sys-locale`), French fallback. Technical error suffixes (`{e}` from
//! libraries) stay raw; only the human prefix is translated.

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use crate::state::app_data_dir;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Lang {
    #[default]
    Fr,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Fr => "fr",
            Lang::En => "en",
        }
    }

    pub fn from_code(s: &str) -> Self {
        if s.trim().to_ascii_lowercase().starts_with("en") {
            Lang::En
        } else {
            Lang::Fr
        }
    }
}

/// Managed state: current UI language.
#[derive(Default)]
pub struct AppLang(pub Mutex<Lang>);

fn lang_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("lang.txt"))
}

/// Language at startup: persisted choice, else OS locale, else French.
pub fn start_lang(app: &AppHandle) -> Lang {
    if let Ok(p) = lang_file(app) {
        if let Ok(s) = std::fs::read_to_string(&p) {
            return Lang::from_code(&s);
        }
    }
    Lang::from_code(sys_locale::get_locale().as_deref().unwrap_or(""))
}

pub fn lang_of(app: &AppHandle) -> Lang {
    app.state::<AppLang>()
        .0
        .lock()
        .map(|g| *g)
        .unwrap_or_default()
}

/// Tray menu labels: (show, toggle, open_ui, quit).
pub fn tray_labels(lang: Lang) -> [&'static str; 4] {
    match lang {
        Lang::Fr => [
            "Afficher",
            "Démarrer / Arrêter le serveur",
            "Ouvrir l'UI web",
            "Quitter",
        ],
        Lang::En => ["Show", "Start / Stop server", "Open web UI", "Quit"],
    }
}

/// User-facing messages (data carried along, rendered per language).
/// (`GpuMemUnavailable` is only constructed off-Windows and in tests.)
#[allow(dead_code)]
pub enum Msg {
    AppDataUnavailable(String),
    BackendNotInstalled,
    BackendTagNotInstalled { tag: String, flavor: String },
    BackendNotFound { tag: String, flavor: String },
    BackendPathOutside,
    ServerRunningStopFirst,
    ZipEntryOutside,
    ZipEntryUnsafe,
    NoModelSelected,
    LaunchFailed(String),
    /// Router mode: llama-server refused the generated preset.
    /// Carries the offending detail extracted from its log.
    PresetRejected(String),
    RouterNoModel,
    MetricsHint(String),
    MetricsStatus(String),
    ModelsPrefix(String),
    ModelsStatus(String),
    GithubStatus(String),
    Download(String),
    HttpStatus(String),
    Stream(String),
    RepoPrivate(String),
    ModelsPathOutside,
    GpuNoneDxgi,
    PdhCounterUnavailable,
    PdhCollect(u8),
    PdhNoInstance,
    PdhRead,
    GpuMemUnavailable,
    BuildUnknown,
}

pub fn msg(lang: Lang, m: Msg) -> String {
    let en = lang == Lang::En;
    match m {
        Msg::AppDataUnavailable(e) => {
            if en {
                format!("app_data folder unavailable: {e}")
            } else {
                format!("dossier app_data indisponible : {e}")
            }
        }
        Msg::BackendNotInstalled => {
            if en {
                "backend not installed".to_string()
            } else {
                "backend non installé".to_string()
            }
        }
        Msg::BackendTagNotInstalled { tag, flavor } => {
            if en {
                format!("backend {tag}/{flavor} not installed")
            } else {
                format!("backend {tag}/{flavor} non installé")
            }
        }
        Msg::BackendNotFound { tag, flavor } => {
            if en {
                format!("backend {tag}/{flavor} not found")
            } else {
                format!("backend {tag}/{flavor} introuvable")
            }
        }
        Msg::BackendPathOutside => {
            if en {
                "path outside the backend folder".to_string()
            } else {
                "chemin hors zone backend".to_string()
            }
        }
        Msg::ServerRunningStopFirst => {
            if en {
                "the server is running: stop it before deleting a backend".to_string()
            } else {
                "le serveur tourne : arrêtez-le avant de supprimer un backend".to_string()
            }
        }
        Msg::ZipEntryOutside => {
            if en {
                "zip: entry outside the backend folder".to_string()
            } else {
                "zip : entrée hors zone backend".to_string()
            }
        }
        Msg::ZipEntryUnsafe => {
            if en {
                "zip: entry outside zone (absolute path or ..)".to_string()
            } else {
                "zip : entrée hors zone (chemin absolu ou ..)".to_string()
            }
        }
        Msg::NoModelSelected => {
            if en {
                "no model selected".to_string()
            } else {
                "aucun modèle sélectionné".to_string()
            }
        }
        Msg::LaunchFailed(e) => {
            if en {
                format!("launch failed: {e}")
            } else {
                format!("lancement : {e}")
            }
        }
        Msg::PresetRejected(e) => {
            if en {
                format!("router config rejected: {e}")
            } else {
                format!("configuration du routeur refusée : {e}")
            }
        }
        Msg::RouterNoModel => {
            if en {
                "no model registered in the router".to_string()
            } else {
                "aucun modèle enregistré dans le routeur".to_string()
            }
        }
        Msg::MetricsHint(e) => {
            if en {
                format!("/metrics: {e} (is the server started with --metrics?)")
            } else {
                format!("/metrics : {e} (le serveur est-il lancé avec --metrics ?)")
            }
        }
        Msg::MetricsStatus(e) => {
            if en {
                format!("/metrics status: {e}")
            } else {
                format!("statut /metrics : {e}")
            }
        }
        Msg::ModelsPrefix(e) => {
            if en {
                format!("models: {e}")
            } else {
                format!("modèles : {e}")
            }
        }
        Msg::ModelsStatus(e) => {
            if en {
                format!("models status: {e}")
            } else {
                format!("statut modèles : {e}")
            }
        }
        Msg::GithubStatus(e) => {
            if en {
                format!("GitHub API status: {e}")
            } else {
                format!("statut GitHub API : {e}")
            }
        }
        Msg::Download(e) => {
            if en {
                format!("download: {e}")
            } else {
                format!("téléchargement : {e}")
            }
        }
        Msg::HttpStatus(e) => {
            if en {
                format!("HTTP status: {e}")
            } else {
                format!("statut HTTP : {e}")
            }
        }
        Msg::Stream(e) => {
            if en {
                format!("stream: {e}")
            } else {
                format!("flux : {e}")
            }
        }
        Msg::RepoPrivate(e) => {
            if en {
                format!("repo not found or private: {e}")
            } else {
                format!("repo introuvable ou privé : {e}")
            }
        }
        Msg::ModelsPathOutside => {
            if en {
                "path outside the models folder".to_string()
            } else {
                "chemin hors du dossier modèles".to_string()
            }
        }
        Msg::GpuNoneDxgi => {
            if en {
                "no GPU detected (DXGI)".to_string()
            } else {
                "aucun GPU détecté (DXGI)".to_string()
            }
        }
        Msg::PdhCounterUnavailable => {
            if en {
                "GPU counter unavailable (PDH)".to_string()
            } else {
                "compteur GPU indisponible (PDH)".to_string()
            }
        }
        Msg::PdhCollect(n) => {
            if en {
                format!("PDH collect {n}")
            } else {
                format!("PDH collecte {n}")
            }
        }
        Msg::PdhNoInstance => {
            if en {
                "PDH: no GPU instance".to_string()
            } else {
                "PDH : aucune instance GPU".to_string()
            }
        }
        Msg::PdhRead => {
            if en {
                "PDH read".to_string()
            } else {
                "PDH lecture".to_string()
            }
        }
        Msg::GpuMemUnavailable => {
            if en {
                "GPU memory unavailable off Windows".to_string()
            } else {
                "mémoire GPU indisponible hors Windows".to_string()
            }
        }
        Msg::BuildUnknown => {
            if en {
                "unknown".to_string()
            } else {
                "inconnue".to_string()
            }
        }
    }
}

/// Frontend-driven language switch: persists, and relabels the tray menu.
/// `menu` is injected by Tauri (invisible to the frontend `invoke`).
#[tauri::command]
pub fn set_app_language(
    app: AppHandle,
    menu: tauri::State<'_, crate::TrayMenu>,
    lang: String,
) -> Result<String, String> {
    let l = Lang::from_code(&lang);
    *app.state::<AppLang>()
        .0
        .lock()
        .map_err(|e| e.to_string())? = l;
    // Best effort: a lost file only falls back to the OS locale once.
    if let Ok(p) = lang_file(&app) {
        let _ = std::fs::write(p, l.code());
    }
    menu.apply_labels(l).map_err(|e| e.to_string())?;
    Ok(l.code().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_msgs() -> Vec<Msg> {
        vec![
            Msg::AppDataUnavailable("x".into()),
            Msg::BackendNotInstalled,
            Msg::BackendTagNotInstalled { tag: "t".into(), flavor: "f".into() },
            Msg::BackendNotFound { tag: "t".into(), flavor: "f".into() },
            Msg::BackendPathOutside,
            Msg::ServerRunningStopFirst,
            Msg::ZipEntryOutside,
            Msg::ZipEntryUnsafe,
            Msg::NoModelSelected,
            Msg::LaunchFailed("x".into()),
            Msg::MetricsHint("x".into()),
            Msg::MetricsStatus("x".into()),
            Msg::ModelsPrefix("x".into()),
            Msg::ModelsStatus("x".into()),
            Msg::GithubStatus("x".into()),
            Msg::Download("x".into()),
            Msg::HttpStatus("x".into()),
            Msg::Stream("x".into()),
            Msg::RepoPrivate("x".into()),
            Msg::ModelsPathOutside,
            Msg::BuildUnknown,
            Msg::GpuNoneDxgi,
            Msg::PdhCounterUnavailable,
            Msg::PdhCollect(1),
            Msg::PdhNoInstance,
            Msg::PdhRead,
            Msg::GpuMemUnavailable,
        ]
    }

    #[test]
    fn every_message_is_translated() {
        // Adding a `Msg` variant without updating this list fails to
        // compile (non-exhaustive match below) — same for the table.
        for (i, m) in all_msgs().into_iter().enumerate() {
            let fr = msg(Lang::Fr, m);
            assert!(!fr.is_empty(), "empty FR message #{i}");
            let en = msg(
                Lang::En,
                all_msgs().into_iter().nth(i).unwrap_or(Msg::BackendNotInstalled),
            );
            assert!(!en.is_empty(), "empty EN message #{i}");
        }
    }

    #[test]
    fn lang_codes_parse() {
        assert_eq!(Lang::from_code("en-US"), Lang::En);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("fr-FR"), Lang::Fr);
        assert_eq!(Lang::from_code(""), Lang::Fr);
        assert_eq!(Lang::from_code("de"), Lang::Fr);
        assert_eq!(Lang::En.code(), "en");
    }

    #[test]
    fn tray_labels_come_in_fours() {
        assert_eq!(tray_labels(Lang::Fr).len(), 4);
        assert_eq!(tray_labels(Lang::En).len(), 4);
        assert!(tray_labels(Lang::Fr)[1].contains("Démarrer"));
    }
}
