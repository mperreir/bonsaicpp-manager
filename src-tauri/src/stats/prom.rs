//! /metrics parsing: counters out of the Prometheus text format.

use tauri::AppHandle;

use crate::locale::{lang_of, msg, Msg};
use super::types::{Ctr, PromptMetrics};

pub(crate) fn read_ctrs(mtext: &str) -> Ctr {
    Ctr {
        pt: parse_metric(mtext, "llamacpp:prompt_tokens_total").unwrap_or(0.0),
        ps: parse_metric(mtext, "llamacpp:prompt_seconds_total").unwrap_or(0.0),
        dt: parse_metric(mtext, "llamacpp:tokens_predicted_total").unwrap_or(0.0),
        ds: parse_metric(mtext, "llamacpp:tokens_predicted_seconds_total").unwrap_or(0.0),
        ct: parse_metric(mtext, "llamacpp:prompt_tokens_cached_total").unwrap_or(0.0),
        sd: parse_metric(mtext, "llamacpp:spec_decode_num_draft_tokens_total").unwrap_or(0.0),
        sa: parse_metric(mtext, "llamacpp:spec_decode_num_accepted_tokens_total").unwrap_or(0.0),
    }
}

/// Coherent (cached, prompt-size) pair over a counter window: the fork
/// counts evaluated-only in `prompt_tokens_total`, so the prompt size
/// is evaluated + cached (ratio always <= 1). `(None, None)` when the
/// window saw no evaluated tokens. Pure for testability.

pub(crate) fn parse_metric(text: &str, name: &str) -> Option<f64> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        // NB: `continue` (not `?`): most lines don't match, and an
        // early `?` would abort the whole scan at the first mismatch.
        let rest = match line.strip_prefix(name) {
            Some(r) => r,
            None => continue,
        };
        if !(rest.is_empty() || rest.starts_with([' ', '\t', '{'])) {
            continue;
        }
        if let Some(last) = line.split_whitespace().last() {
            if let Ok(v) = last.parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

pub(crate) fn parse_metric_u64(text: &str, name: &str) -> Option<u64> {
    parse_metric(text, name).map(|v| v as u64)
}

#[tauri::command]
pub async fn scrape_metrics(app: AppHandle, port: u16) -> Result<PromptMetrics, String> {
    let url = format!("http://127.0.0.1:{}/metrics", port);
    let lang = lang_of(&app);
    let text = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| msg(lang, Msg::MetricsHint(e.to_string())))?
        .error_for_status()
        .map_err(|e| msg(lang, Msg::MetricsStatus(e.to_string())))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    Ok(PromptMetrics {
        prompt_tps: parse_metric(&text, "llamacpp:prompt_tokens_seconds"),
        predicted_tps: parse_metric(&text, "llamacpp:predicted_tokens_seconds"),
        requests_processing: parse_metric_u64(&text, "llamacpp:requests_processing"),
        requests_deferred: parse_metric_u64(&text, "llamacpp:requests_deferred"),
        tokens_cache_used: parse_metric_u64(&text, "llamacpp:tokens_cache_used"),
        raw_scrape_ok: true,
    })
}


