//! Router mode: model registry + generation of the `llama-server`
//! `--models-preset` INI file.
//!
//! Everything here is derived from the `--help` schema discovered at
//! launch (`help_parser::HelpSchema`) — no option name is hardcoded:
//! `validate` rejects any key that the installed binary does not know,
//! because llama.cpp refuses to start on an unknown preset key:
//! ```text
//! E srv  llama_server: failed to initialize router models:
//!    option 'this-flag-does-not-exist' not recognized in preset 'x'
//! ```
//!
//! Two-level INI, both levels using the same `--help` vocabulary:
//! - `[*]`  : global defaults shared by every model
//! - `[alias]` : one model + its overrides (+ the preset-only
//!   `load-on-startup` key)
//!
//! Precedence (llama.cpp): CLI args > `[alias]` > `[*]`. Some keys are
//! owned by the router itself and are dropped or overwritten on load
//! (`host`, `port`, `api-key`, `hf-repo`, `alias`): they are never
//! written, and `validate` rejects them with a dedicated message.

use serde::{Deserialize, Serialize};

use crate::help_parser::HelpSchema;

/// Preset-only keys: they are *not* command-line arguments, so they can
/// never be found in the `--help` schema. Kept here explicitly.
pub const LOAD_ON_STARTUP: &str = "load-on-startup";

/// Keys the router owns. Writing them is not fatal (llama.cpp silently
/// overrides them with the CLI value) but it would be misleading, so the
/// generator never emits them.
const ROUTER_CONTROLLED: &[&str] = &[
    "host",
    "port",
    "api-key",
    "api-key-file",
    "alias",
    "hf-repo",
    "hf-file",
    "model",
    "models-dir",
    "models-preset",
    "models-max",
    "models-autoload",
    "no-models-autoload",
    "sleep-idle-seconds",
];

/// Preset-only keys: no command-line counterpart, hence no `--help`
/// entry. They are emitted from typed fields, never from the override
/// pairs, so the generic writer drops them.
const PRESET_ONLY: &[&str] = &["stop-timeout", "dedup-cache-models"];

/// `--ctx-size` -> `ctx-size`, `-t` -> `t`.
pub(crate) fn ini_key(flag: &str) -> String {
    flag.trim_start_matches('-').to_string()
}

/// True for CLI keys the router owns (never written to the preset).
pub fn is_router_controlled(key: &str) -> bool {
    let k = ini_key(key);
    ROUTER_CONTROLLED.iter().any(|c| ini_key(c) == k)
}

/// Is this key one of the preset-only keys (no `--help` counterpart)?
/// `load-on-startup` is the one we surface, as a typed field.
pub fn is_preset_only(key: &str) -> bool {
    let k = ini_key(key);
    k == LOAD_ON_STARTUP || PRESET_ONLY.iter().any(|p| ini_key(p) == k)
}

/// One model known to the manager. Every downloaded GGUF has an entry:
/// `routed` decides whether it is published in the preset (visible to API
/// clients) or kept aside (its alias/overrides survive an uncheck, so
/// re-exposing a model restores its previous settings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterModel {
    /// Short name used by API clients in the `model` field.
    pub alias: String,
    /// Absolute path of the GGUF (also the INI section name source).
    pub model_path: String,
    /// Expose this model through the router.
    #[serde(default = "default_true")]
    pub routed: bool,
    /// Per-model overrides, empty value = bare flag (`jinja = true`).
    pub overrides: Vec<(String, String)>,
    /// Load this model when the server starts (default: no).
    #[serde(default)]
    pub load_on_startup: bool,
}

fn default_true() -> bool {
    true
}

impl RouterModel {
    #[allow(dead_code)] // used by tests and by future registry helpers
    pub fn new(alias: impl Into<String>, model_path: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            model_path: model_path.into(),
            routed: true,
            overrides: Vec::new(),
            load_on_startup: false,
        }
    }
}

/// Global router settings (CLI) + the `[*]` section of the preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    #[serde(default)]
    pub models: Vec<RouterModel>,
    /// `[*]` section: defaults shared by every model.
    #[serde(default)]
    pub global: Vec<(String, String)>,
    /// `--models-max` (0 = unlimited).
    #[serde(default = "default_models_max")]
    pub models_max: i32,
    /// `--sleep-idle-seconds` (-1 = disabled). Per model instance.
    #[serde(default = "default_sleep_idle")]
    pub sleep_idle_seconds: i32,
}

fn default_models_max() -> i32 {
    4
}
fn default_sleep_idle() -> i32 {
    -1
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            global: Vec::new(),
            models_max: default_models_max(),
            sleep_idle_seconds: default_sleep_idle(),
        }
    }
}

impl RouterConfig {
    /// Router mode is active as soon as one model is *routed*.
    pub fn is_router(&self) -> bool {
        self.models.iter().any(|m| m.routed)
    }

    /// The models actually published in the preset.
    pub fn routed(&self) -> impl Iterator<Item = &RouterModel> {
        self.models.iter().filter(|m| m.routed)
    }

    #[allow(dead_code)] // used by tests
    pub fn find(&self, alias: &str) -> Option<&RouterModel> {
        self.models.iter().find(|m| m.alias == alias)
    }

    /// Reconcile the registry with the GGUF files on disk: every local file
    /// gets an entry (un-routed by default is *not* applied — a model the
    /// user had exposed stays exposed), and entries whose file disappeared
    /// are dropped. `alias_of` proposes the initial alias.
    pub fn sync_with(&self, local: &[String], mut alias_of: impl FnMut(&str) -> String) -> RouterConfig {
        let mut out = self.clone();
        // Drop vanished files.
        out.models.retain(|m| local.iter().any(|p| *p == m.model_path));
        // Add the new ones, with a non-conflicting alias.
        for path in local {
            if out.models.iter().any(|m| m.model_path == *path) {
                continue;
            }
            let base = alias_of(path);
            let mut alias = base.clone();
            let mut n = 2;
            while out.models.iter().any(|m| m.alias == alias) {
                alias = format!("{base}-{n}");
                n += 1;
            }
            out.models.push(RouterModel {
                alias,
                model_path: path.clone(),
                routed: false,
                overrides: Vec::new(),
                load_on_startup: false,
            });
        }
        out
    }

    /// Serialize to the INI consumed by `--models-preset`.
    /// Only routed models get a section; ordering is deterministic (sorted
    /// keys, registry order) so the file is stable across writes.
    pub fn render_ini(&self) -> String {
        let mut out = String::from("version = 1\n");

        let globals = filtered_pairs(&self.global);
        if !globals.is_empty() {
            out.push_str("\n; global defaults, shared by every model ([*])\n[*]\n");
            for (k, v) in &globals {
                out.push_str(&format!("{} = {}\n", k, v));
            }
        }

        for m in self.routed() {
            out.push_str(&format!(
                "\n; {}\n[{}]\nmodel = {}\n",
                m.alias, m.alias, m.model_path
            ));
            for (k, v) in filtered_pairs(&m.overrides) {
                out.push_str(&format!("{} = {}\n", k, v));
            }
            if m.load_on_startup {
                out.push_str(&format!("{LOAD_ON_STARTUP} = true\n"));
            }
        }
        out
    }
}

/// Normalize + drop keys the router owns (never written) and the
/// preset-only keys (written from their typed field instead).
/// Sorted for a stable file.
fn filtered_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = pairs
        .iter()
        .filter(|(k, _)| {
            !k.is_empty() && !is_router_controlled(k) && !is_preset_only(k)
        })
        .map(|(k, val)| (ini_key(k), val.trim().to_string()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

/// A rejected key, with enough context for a translated UI message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IniError {
    /// Where the key was declared: `"*"` or an alias.
    pub scope: String,
    pub key: String,
    pub reason: IniErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IniErrorKind {
    /// Not present in the `llama-server --help` schema (fatal at launch).
    UnknownKey,
    /// Router-controlled: it would be overwritten anyway.
    RouterControlled,
    /// Preset-only key: it has a dedicated setting, not an override.
    PresetOnly,
}

/// Validate every key of the config against the discovered schema.
pub fn validate(cfg: &RouterConfig, schema: &HelpSchema) -> Vec<IniError> {
    let mut errs = Vec::new();
    let mut check = |pairs: &[(String, String)], scope: &str| {
        for (k, _) in pairs {
            let key = ini_key(k);
            if key.is_empty() {
                continue;
            }
            let reason = if is_router_controlled(&key) {
                IniErrorKind::RouterControlled
            } else if is_preset_only(&key) {
                IniErrorKind::PresetOnly
            } else if !schema_known(schema, &key) {
                IniErrorKind::UnknownKey
            } else {
                continue;
            };
            errs.push(IniError {
                scope: scope.to_string(),
                key: key.clone(),
                reason,
            });
        }
    };
    check(&cfg.global, "*");
    // Un-routed models never reach the preset: their overrides are inert.
    for m in cfg.routed() {
        check(&m.overrides, &m.alias);
    }
    errs
}

/// Does the schema know this INI key? Accepts the canonical long flag
/// and any long alias; short forms are deliberately rejected by
/// `render_ini` (we always write canonical names) but tolerated here.
fn schema_known(schema: &HelpSchema, key: &str) -> bool {
    let wanted = format!("--{key}");
    schema.options.iter().any(|o| {
        o.flag == wanted || o.aliases.iter().any(|a| a == &wanted) || ini_key(&o.flag) == key
    })
}

/// Why a model alias cannot be used (INI section names).
pub fn alias_error(alias: &str) -> Option<&'static str> {
    if alias.trim().is_empty() {
        return Some("empty");
    }
    if alias.contains(['[', ']', '\n', '\r']) {
        return Some("chars");
    }
    None
}

// ---------------------------------------------------------------------------
// Persistence: <appdata>/config/router.json + <appdata>/config/models.ini
// ---------------------------------------------------------------------------

pub fn config_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::state::app_data_dir(app)?.join("config"))
}

pub fn preset_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(config_dir(app)?.join("models.ini"))
}

pub fn config_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(config_dir(app)?.join("router.json"))
}

/// Read the registry (defaults when absent or unreadable).
pub fn load(app: &tauri::AppHandle) -> RouterConfig {
    let Ok(p) = config_path(app) else {
        return RouterConfig::default();
    };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str::<RouterConfig>(&t).ok())
        .unwrap_or_default()
}

/// Persist the registry (JSON).
pub fn save(app: &tauri::AppHandle, cfg: &RouterConfig) -> Result<(), String> {
    let dir = config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app)?, json).map_err(|e| e.to_string())
}

/// Write the preset consumed by `--models-preset`.
pub fn write_preset(app: &tauri::AppHandle, cfg: &RouterConfig) -> Result<std::path::PathBuf, String> {
    let dir = config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = preset_path(app)?;
    std::fs::write(&p, cfg.render_ini()).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Load the registry (frontend bootstrap).
#[tauri::command]
pub async fn router_config(app: tauri::AppHandle) -> Result<RouterConfig, String> {
    Ok(load(&app))
}

/// Validate against the live `--help` schema of the installed backend.
#[tauri::command]
pub async fn router_validate(
    app: tauri::AppHandle,
    config: RouterConfig,
) -> Result<Vec<IniError>, String> {
    let schema = crate::help_parser::fetch_schema(&app).await?;
    Ok(validate(&config, &schema))
}

/// Validate, persist and regenerate the preset. Returns the list of
/// rejected keys; nothing is written when the config is invalid, so the
/// server can never be started from a preset llama.cpp would refuse.
#[tauri::command]
pub async fn set_router_config(
    app: tauri::AppHandle,
    config: RouterConfig,
) -> Result<Vec<IniError>, String> {
    // Structural checks first (no schema needed).
    let mut errs: Vec<IniError> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for m in config.routed() {
        if let Some(why) = alias_error(&m.alias) {
            errs.push(IniError {
                scope: m.alias.clone(),
                key: m.alias.clone(),
                reason: if why == "empty" {
                    IniErrorKind::RouterControlled
                } else {
                    IniErrorKind::UnknownKey
                },
            });
            continue;
        }
        if seen.contains(&m.alias.as_str()) {
            errs.push(IniError {
                scope: m.alias.clone(),
                key: m.alias.clone(),
                reason: IniErrorKind::RouterControlled,
            });
            continue;
        }
        seen.push(&m.alias);
        if m.model_path.trim().is_empty() {
            errs.push(IniError {
                scope: m.alias.clone(),
                key: "model".to_string(),
                reason: IniErrorKind::UnknownKey,
            });
        }
    }
    if !errs.is_empty() {
        return Ok(errs);
    }
    let schema = crate::help_parser::fetch_schema(&app).await?;
    errs = validate(&config, &schema);
    if !errs.is_empty() {
        return Ok(errs);
    }
    save(&app, &config)?;
    write_preset(&app, &config)?;
    Ok(errs)
}

/// Apply the freshly written preset to a **running** router, without a
/// restart.
///
/// Measured on `prism-b10772`: after rewriting the INI,
/// `GET /models?reload=1` re-reads it and
/// - adds / removes / renames the sections,
/// - **unloads** a model whose source changed or disappeared
///   (`load_models: (reload) unloading model name=alpha (source updated or removed)`),
///
/// so an alias edit is served under the new alias on the very next request,
/// with no restart. That covers everything written in the preset: exposure,
/// aliases, per-model overrides, `load-on-startup` and the `[*]` defaults.
///
/// It does **not** cover `--models-max` / `--sleep-idle-seconds`: those are
/// command-line flags owned by the router, so they need a real restart (the
/// UI says so).
#[tauri::command]
pub async fn reload_router_models(port: u16) -> Result<Vec<(String, String)>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/models?reload=1"))
        .send()
        .await
        .map_err(|e| format!("/models?reload=1: {e}"))?
        .error_for_status()
        .map_err(|e| format!("/models?reload=1 statut: {e}"))?
        .json()
        .await
        .map_err(|e| format!("/models?reload=1 JSON: {e}"))?;
    Ok(crate::stats::endpoint::parse_models_list(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help_parser::{parse_help, OptionKind};

    /// Minimal schema with the flags used across the tests.
    fn schema() -> HelpSchema {
        parse_help(
            "----- common params -----\n\
             -t,    --threads N          number of CPU threads (default: -1)\n\
             -c,    --ctx-size N         size of the prompt context (default: 0)\n\
             -ngl,  --n-gpu-layers N     max layers in VRAM (default: auto)\n\
             -fa,   --flash-attn [on|off|auto]  set Flash Attention use\n\
             -m,    --model FNAME        model path to load\n\
             ----- example-specific params -----\n\
             --host HOST                IP addresses to listen on (default: 127.0.0.1)\n\
             --port PORT                port to listen (default: 9931)\n\
             --jinja, --no-jinja         whether to use jinja template engine\n\
             --kv-unified, -no-kvu, --no-kv-unified  use single unified KV buffer\n",
        )
    }

    fn cfg_with(pairs: &[(&str, &str)]) -> RouterConfig {
        RouterConfig {
            global: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ini_key_strips_dashes() {
        assert_eq!(ini_key("--ctx-size"), "ctx-size");
        assert_eq!(ini_key("-t"), "t");
        assert_eq!(ini_key("ctx-size"), "ctx-size");
    }

    #[test]
    fn render_ini_writes_version_globals_and_sections() {
        let mut cfg = cfg_with(&[("--flash-attn", "true"), ("--ctx-size", "4096")]);
        let mut m = RouterModel::new("bonsai-27b", "C:/models/27b.gguf");
        m.overrides = vec![("--ctx-size".into(), "8192".into())];
        m.load_on_startup = true;
        cfg.models.push(m);

        let ini = cfg.render_ini();
        assert!(ini.starts_with("version = 1\n"));
        assert!(ini.contains("[*]"));
        assert!(ini.contains("ctx-size = 4096"));
        assert!(ini.contains("[bonsai-27b]"));
        assert!(ini.contains("model = C:/models/27b.gguf"));
        // Per-model override wins inside its own section.
        assert!(ini.contains("ctx-size = 8192"));
        assert!(ini.contains("load-on-startup = true"));
    }

    #[test]
    fn render_ini_is_deterministic() {
        let mut cfg = cfg_with(&[("--threads", "8"), ("--ctx-size", "4096")]);
        cfg.models.push(RouterModel::new("a", "a.gguf"));
        let once = cfg.render_ini();
        let twice = cfg.render_ini();
        assert_eq!(once, twice);
        // sorted: ctx-size before threads
        assert!(once.find("ctx-size").unwrap() < once.find("threads").unwrap());
    }

    #[test]
    fn render_ini_never_writes_router_controlled_keys() {
        let mut cfg = cfg_with(&[("--port", "1234"), ("--host", "0.0.0.0")]);
        let mut m = RouterModel::new("a", "a.gguf");
        m.overrides = vec![("port".into(), "9999".into()), ("alias".into(), "zz".into())];
        cfg.models.push(m);
        let ini = cfg.render_ini();
        assert!(!ini.contains("1234"));
        assert!(!ini.contains("0.0.0.0"));
        assert!(!ini.contains("9999"));
        assert!(!ini.contains("zz"));
    }

    #[test]
    fn validate_accepts_known_flags() {
        let mut cfg = cfg_with(&[("--flash-attn", "true")]);
        let mut m = RouterModel::new("a", "a.gguf");
        m.overrides = vec![("--ctx-size".into(), "8192".into())];
        cfg.models.push(m);
        assert!(validate(&cfg, &schema()).is_empty());
    }

    #[test]
    fn validate_rejects_unknown_key_with_scope() {
        let cfg = cfg_with(&[("--not-a-real-flag", "1")]);
        let errs = validate(&cfg, &schema());
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].key, "not-a-real-flag");
        assert_eq!(errs[0].scope, "*");
        assert_eq!(errs[0].reason, IniErrorKind::UnknownKey);
    }

    #[test]
    fn validate_flags_router_controlled_separately() {
        let cfg = cfg_with(&[("--port", "1")]);
        let errs = validate(&cfg, &schema());
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].reason, IniErrorKind::RouterControlled);
    }

    #[test]
    fn validate_accepts_long_aliases_from_schema() {
        // `--n-gpu-layers` is the canonical flag of the `-ngl` short form.
        let cfg = cfg_with(&[("--n-gpu-layers", "99")]);
        assert!(validate(&cfg, &schema()).is_empty());
    }

    #[test]
    fn is_router_controlled_covers_router_flags() {
        assert!(is_router_controlled("--port"));
        assert!(is_router_controlled("port"));
        assert!(is_router_controlled("--models-max"));
        assert!(is_router_controlled("--sleep-idle-seconds"));
        assert!(!is_router_controlled("--ctx-size"));
    }

    #[test]
    fn preset_only_keys_are_separate_from_router_controlled() {
        // `load-on-startup` must still be emitted by render_ini (it comes
        // from the typed field), so it is neither router-controlled nor a
        // generic override.
        assert!(is_preset_only(LOAD_ON_STARTUP));
        assert!(!is_router_controlled(LOAD_ON_STARTUP));
        assert!(is_preset_only("stop-timeout"));
        assert!(!is_preset_only("ctx-size"));
    }

    #[test]
    fn validate_reports_preset_only_key_in_overrides() {
        let mut cfg = RouterConfig::default();
        let mut m = RouterModel::new("a", "a.gguf");
        m.overrides = vec![(LOAD_ON_STARTUP.into(), "true".into())];
        cfg.models.push(m);
        let errs = validate(&cfg, &schema());
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].reason, IniErrorKind::PresetOnly);
        assert_eq!(errs[0].scope, "a");
    }

    #[test]
    fn render_ini_drops_preset_only_keys_from_pairs() {
        // Only the typed field may emit load-on-startup.
        let mut cfg = RouterConfig::default();
        let mut m = RouterModel::new("a", "a.gguf");
        m.overrides = vec![(LOAD_ON_STARTUP.into(), "false".into())];
        cfg.models.push(m);
        let ini = cfg.render_ini();
        assert!(!ini.contains(LOAD_ON_STARTUP));
    }

    #[test]
    fn alias_validation_rejects_bad_section_names() {
        assert_eq!(alias_error(""), Some("empty"));
        assert_eq!(alias_error("  "), Some("empty"));
        assert_eq!(alias_error("a[b]"), Some("chars"));
        assert_eq!(alias_error("a\nb"), Some("chars"));
        assert_eq!(alias_error("bonsai-27b"), None);
    }

    #[test]
    fn is_router_true_only_with_models() {
        let mut cfg = RouterConfig::default();
        assert!(!cfg.is_router());
        cfg.models.push(RouterModel::new("a", "a.gguf"));
        assert!(cfg.is_router());
        assert!(cfg.find("a").is_some());
        assert!(cfg.find("zzz").is_none());
    }

    #[test]
    fn unrouted_models_are_absent_from_the_ini() {
        let mut cfg = RouterConfig::default();
        let mut shown = RouterModel::new("shown", "shown.gguf");
        shown.overrides = vec![("ctx-size".into(), "8192".into())];
        let mut hidden = RouterModel::new("hidden", "hidden.gguf");
        hidden.routed = false;
        hidden.overrides = vec![("ctx-size".into(), "4096".into())];
        cfg.models = vec![shown, hidden];

        let ini = cfg.render_ini();
        assert!(ini.contains("[shown]"));
        assert!(!ini.contains("[hidden]"));
        assert!(!ini.contains("4096"));
        assert_eq!(cfg.routed().count(), 1);
    }

    #[test]
    fn unrouted_models_are_not_validated() {
        // An un-routed model's overrides never reach the preset, so a bad
        // key there must not block saving (it would only bite later, once
        // the user ticks the box — and validate() catches it then).
        let mut cfg = RouterConfig::default();
        let mut hidden = RouterModel::new("hidden", "hidden.gguf");
        hidden.routed = false;
        hidden.overrides = vec![("bogus-key".into(), "1".into())];
        cfg.models = vec![hidden];
        assert!(validate(&cfg, &schema()).is_empty());
    }

    #[test]
    fn sync_adds_new_files_as_unrouted_and_drops_vanished_ones() {
        let mut cfg = RouterConfig::default();
        cfg.models = vec![RouterModel::new("old", "old.gguf")];

        let synced = cfg.sync_with(
            &["new.gguf".to_string(), "old.gguf".to_string()],
            |p| p.trim_end_matches(".gguf").to_string(),
        );
        // The vanished file is gone; the exposed one keeps its alias and
        // stays exposed; the newly discovered one is listed but *not*
        // exposed (adding a model must never publish it silently).
        assert_eq!(synced.models.len(), 2);
        assert!(synced.find("old").unwrap().routed);
        assert!(!synced.find("new").unwrap().routed);
        assert!(synced.is_router());
        assert!(!synced.render_ini().contains("[new]"));
    }

    #[test]
    fn sync_on_a_fresh_config_exposes_nothing() {
        let cfg = RouterConfig::default();
        let synced = cfg.sync_with(
            &["a.gguf".to_string(), "b.gguf".to_string()],
            |p| p.trim_end_matches(".gguf").to_string(),
        );
        assert_eq!(synced.models.len(), 2);
        assert!(!synced.is_router());
        // Only `version = 1` then: an empty preset, nothing published.
        assert_eq!(synced.render_ini().trim(), "version = 1");
    }

    #[test]
    fn sync_never_duplicates_an_alias() {
        // Two files whose basename-derived alias collides.
        let cfg = RouterConfig::default();
        let synced = cfg.sync_with(&["a.gguf".to_string(), "b/a.gguf".to_string()], |p| {
            p.rsplit('/')
                .next()
                .unwrap_or(p)
                .trim_end_matches(".gguf")
                .to_string()
        });
        let aliases: Vec<&str> = synced.models.iter().map(|m| m.alias.as_str()).collect();
        assert_eq!(aliases.len(), 2);
        for (i, a) in aliases.iter().enumerate() {
            assert!(
                !aliases[i + 1..].contains(a),
                "duplicate alias {a} in {aliases:?}"
            );
        }
    }

    #[test]
    fn sync_preserves_overrides_across_uncheck_check() {
        let mut cfg = RouterConfig::default();
        let mut m = RouterModel::new("a", "a.gguf");
        m.overrides = vec![("ctx-size".into(), "16384".into())];
        cfg.models = vec![m];

        let unchecked = cfg.sync_with(&["a.gguf".to_string()], |p| p.to_string());
        let mut next = unchecked.clone();
        next.models[0].routed = false;
        // Unchecking then re-checking keeps the alias and the overrides.
        next.models[0].routed = true;
        assert_eq!(next.models[0].overrides, vec![("ctx-size".to_string(), "16384".to_string())]);
        assert!(next.render_ini().contains("ctx-size = 16384"));
    }

    #[test]
    fn schema_option_kinds_are_available_for_validation_ui() {
        // Guards the assumption that kind drives the INI value editor.
        let s = schema();
        let ctx = s
            .options
            .iter()
            .find(|o| o.flag == "--ctx-size")
            .expect("ctx-size parsed");
        assert_eq!(ctx.kind, OptionKind::Int);
    }
}
