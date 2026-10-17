//! Tauri commands for one-off timing capture and the tray tooltip.

use tauri::AppHandle;

use crate::locale::{lang_of, msg, Msg};
use super::sample::{has_speeds, push_history};
use super::types::{StatsState, TimingSample};

#[tauri::command]
pub async fn record_timing(
    state: tauri::State<'_, StatsState>,
    prompt_n: Option<u64>,
    prompt_ms: Option<f64>,
    predicted_n: Option<u64>,
    predicted_ms: Option<f64>,
) -> Result<TimingSample, String> {
    let prompt_tps = match (prompt_n, prompt_ms) {
        (Some(n), Some(ms)) if ms > 0.0 => Some(n as f64 / (ms / 1000.0)),
        _ => None,
    };
    let predicted_tps = match (predicted_n, predicted_ms) {
        (Some(n), Some(ms)) if ms > 0.0 => Some(n as f64 / (ms / 1000.0)),
        _ => None,
    };
    let sample = TimingSample {
        at: chrono::Local::now().to_rfc3339(),
        model: None,
        prompt_n,
        prompt_ms,
        prompt_tps,
        cache_n: None,
        cache_total_n: None,
        predicted_n,
        predicted_ms,
        predicted_tps,
        spec_drafted: None,
        spec_accepted: None,
    };
    if has_speeds(&sample) {
        push_history(&state, sample.clone());
    }
    Ok(sample)
}

/// Send a tiny non-streaming chat request and record its `timings`
/// in the history.
#[tauri::command]
pub async fn send_test_prompt(
    app: AppHandle,
    state: tauri::State<'_, StatsState>,
    port: u16,
) -> Result<TimingSample, String> {
    let lang = lang_of(&app);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let base = format!("http://127.0.0.1:{port}");
    let models: serde_json::Value = client
        .get(format!("{base}/v1/models"))
        .send()
        .await
        .map_err(|e| msg(lang, Msg::ModelsPrefix(e.to_string())))?
        .error_for_status()
        .map_err(|e| msg(lang, Msg::ModelsStatus(e.to_string())))?
        .json()
        .await
        .map_err(|e| format!("modeles JSON: {e}"))?;
    let model_id = models
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(|m| m.get("id"))
        .and_then(|i| i.as_str())
        .unwrap_or("default");
    let body = serde_json::json!({
        "model": model_id,
        "messages": [{"role": "user", "content": "Dis juste bonjour."}],
        "max_tokens": 32,
        "stream": false,
    });
    let resp: serde_json::Value = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("completion: {e}"))?
        .error_for_status()
        .map_err(|e| format!("completion statut: {e}"))?
        .json()
        .await
        .map_err(|e| format!("completion JSON: {e}"))?;
    let t = &resp["timings"];
    let num = |k: &str| t.get(k).and_then(|v| v.as_u64());
    let ms = |k: &str| t.get(k).and_then(|v| v.as_f64());
    let (prompt_n, prompt_ms, predicted_n, predicted_ms) =
        (num("prompt_n"), ms("prompt_ms"), num("predicted_n"), ms("predicted_ms"));
    // `timings` counts the full prompt (cached included).
    let (cache_n, cache_total_n) = match (num("cache_n"), prompt_n) {
        (Some(c), Some(n)) if n > 0 => (Some(c.min(n)), Some(n)),
        _ => (None, None),
    };
    let sample = TimingSample {
        at: chrono::Local::now().to_rfc3339(),
        model: None,
        prompt_n,
        prompt_ms,
        prompt_tps: match (prompt_n, prompt_ms) {
            (Some(n), Some(ms)) if ms > 0.0 => Some(n as f64 / (ms / 1000.0)),
            _ => None,
        },
        cache_n,
        cache_total_n,
        predicted_n,
        predicted_ms,
        predicted_tps: match (predicted_n, predicted_ms) {
            (Some(n), Some(ms)) if ms > 0.0 => Some(n as f64 / (ms / 1000.0)),
            _ => None,
        },
        spec_drafted: num("draft_n"),
        spec_accepted: num("draft_n_accepted"),
    };
    if has_speeds(&sample) {
        push_history(&state, sample.clone());
    }
    Ok(sample)
}

/// Update the tray icon tooltip (plain icon, text only).
#[tauri::command]
pub async fn set_tray_tooltip(app: tauri::AppHandle, text: String) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_tooltip(Some(text)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Live activity.
// ---------------------------------------------------------------------------


