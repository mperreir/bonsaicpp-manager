//! Router HTTP layer: mode detection, the registry, and the
//! per-model ?model=...&autoload=false reads.

use super::types::TimingSample;

// ---------------------------------------------------------------------------
// Router mode helpers.
// ---------------------------------------------------------------------------

/// Is the server in router mode? Without a `model` query param `/props`
/// answers the router itself (`{"role":"router","max_instances":N,...}`),
/// while a single-model server answers its own props.
pub(crate) async fn is_router(client: &reqwest::Client, base: &str) -> bool {
    match client.get(format!("{base}/props")).send().await {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v.get("role").and_then(|r| r.as_str()) == Some("router"),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Percent-encode an alias for the `?model=` query param.
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Registered models + their router status (`unloaded`, `loading`,
/// `loaded`, `sleeping`), from `GET /models`. Pure parser, tested.
pub(crate) fn parse_models_list(v: &serde_json::Value) -> Vec<(String, String)> {
    v.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|x| x.as_str())?.to_string();
                    let st = m
                        .get("status")
                        .and_then(|s| s.get("value"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("unloaded")
                        .to_string();
                    Some((id, st))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/metrics` + `/slots` for one model. `autoload=false` is mandatory: the
/// router *loads* a model on a bare `/slots?model=X`, which would make the
/// poller load every registered model and defeat router mode. With it, an
/// unloaded model answers 400 `model is not loaded` (no side effect) and a
/// sleeping one still answers (idle slots, zeroed counters).
pub(crate) async fn fetch_model(
    client: &reqwest::Client,
    base: &str,
    alias: &str,
) -> (String, Option<serde_json::Value>) {
    let q = format!("model={}&autoload=false", urlencode(alias));
    let mtext = match client.get(format!("{base}/metrics?{q}")).send().await {
        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
        _ => String::new(),
    };
    let slots = match client.get(format!("{base}/slots?{q}")).send().await {
        Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
        _ => None,
    };
    (mtext, slots)
}

/// Find the history row to amend: same `at` **and** same model (rows of
/// several models interleave).
pub(crate) fn hist_row_mut<'a>(h: &'a mut Vec<TimingSample>, at: &str, model: &Option<String>) -> Option<&'a mut TimingSample> {
    h.iter_mut()
        .rev()
        .find(|e| e.at == at && e.model == *model)
}
