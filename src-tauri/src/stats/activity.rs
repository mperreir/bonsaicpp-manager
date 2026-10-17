//! The activity poll command: detects router mode, fetches every model's
//! slots/counters (never autoloading), then reports the selected model.

use super::endpoint::{fetch_model, is_router, parse_models_list};
use super::log::parse_model_ports;
use super::poll::{poll_model, ModelOut};
use super::prom::read_ctrs;
use super::types::{Activity, ModelActivity, StatsState, SINGLE_MODEL_KEY};

/// One activity poll. Router mode is handled transparently: the router's
/// `/props` (no `model` param) tells us it is a router, then every
/// registered model is polled on its own `/metrics?model=` + `/slots?model=`
/// with `autoload=false`, and the state (slots, windows, averages,
/// pending catch-up) is kept per model.
#[tauri::command]
pub async fn poll_activity(
    app: tauri::AppHandle,
    state: tauri::State<'_, StatsState>,
    port: u16,
) -> Result<Activity, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let base = format!("http://127.0.0.1:{port}");

    let router = is_router(&client, &base).await;

    // Registry + status. In single-model mode there is one implicit target.
    let targets: Vec<(String, String)> = if router {
        match client.get(format!("{base}/models")).send().await {
            Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                Ok(v) => parse_models_list(&v),
                Err(_) => vec![],
            },
            _ => vec![],
        }
    } else {
        vec![(SINGLE_MODEL_KEY.to_string(), "loaded".to_string())]
    };

    // Instance port per model, from the router's own log lines: it is what
    // attributes a `print_timing ... task N` line to a model (task ids
    // restart in every instance).
    let log_path = crate::state::app_data_dir(&app)
        .ok()
        .map(|d| d.join("logs").join("llama-server.log"));
    let log_text = log_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let ports = parse_model_ports(&log_text);

    // --- Network phase: every HTTP call happens here, before any lock.
    let mut fetched: Vec<(String, String, String, Option<serde_json::Value>)> =
        Vec::with_capacity(targets.len());
    for (key, status) in &targets {
        let (mtext, slots) = if router {
            fetch_model(&client, &base, key).await
        } else {
            let mtext = match client.get(format!("{base}/metrics")).send().await {
                Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                _ => return Err(format!("/metrics: indisponible sur le port {port}")),
            };
            let slots = match client.get(format!("{base}/slots")).send().await {
                Ok(r) => r.json::<serde_json::Value>().await.ok(),
                Err(_) => None,
            };
            (mtext, slots)
        };
        fetched.push((key.clone(), status.clone(), mtext, slots));
    }

    // --- Synchronous section under lock (no await below).
    let mut track_map = state.track.lock().map_err(|e| e.to_string())?;
    let mut avg_map = state.avg.lock().map_err(|e| e.to_string())?;
    let mut hist = state.hist.lock().map_err(|e| e.to_string())?;
    let now = std::time::Instant::now();

    let mut outs: Vec<(String, String, ModelOut)> = Vec::with_capacity(fetched.len());
    for (key, status, mtext, slots) in &fetched {
        let track = track_map.entry(key.clone()).or_default();
        let avg = avg_map.entry(key.clone()).or_default();
        let ctr_now = read_ctrs(mtext);
        let out = poll_model(
            key,
            track,
            avg,
            &mut hist,
            mtext,
            slots.as_ref(),
            ctr_now,
            log_path.as_deref(),
            if router { ports.get(key.as_str()).copied() } else { None },
            now,
        );
        outs.push((key.clone(), status.clone(), out));
    }
    // Forget models removed from the registry (their history rows stay).
    track_map.retain(|k, _| targets.iter().any(|(t, _)| t == k));
    avg_map.retain(|k, _| targets.iter().any(|(t, _)| t == k));

    // "Auto" selection: a model decoding right now, else the one that most
    // recently produced a finished request, else the first.
    let last_served = hist.last().and_then(|s| s.model.clone());
    let idx = outs
        .iter()
        .position(|(_, _, o)| o.active_slots.iter().any(|s| s.phase == "decode"))
        .or_else(|| {
            outs.iter()
                .position(|(k, _, _)| last_served.as_ref() == Some(k))
        });
    let Some(idx) = idx.filter(|_| !outs.is_empty()) else {
        // Nothing registered yet (or the server stopped answering).
        return Ok(Activity::empty(router));
    };
    let (chosen_key, _, chosen) = &outs[idx];

    let models: Vec<ModelActivity> = outs
        .iter()
        .map(|(k, st, o)| ModelActivity {
            alias: k.clone(),
            status: st.clone(),
            active: o.active_slots.iter().any(|s| s.phase == "decode"),
            prefill_tps: o.prefill_tps,
            decode_tps: o.decode_tps,
            ctx_used: o.ctx_used,
            ctx_max: o.ctx_max,
        })
        .collect();

    let history = hist.clone();
    Ok(Activity {
        router,
        selected_model: if router {
            Some(chosen_key.clone())
        } else {
            None
        },
        models,
        prefill_tps: chosen.prefill_tps,
        decode_tps: chosen.decode_tps,
        avg_prefill_tps: chosen.avg_prefill_tps,
        avg_decode_tps: chosen.avg_decode_tps,
        avg_prefill_polls: chosen.avg_prefill_polls,
        avg_decode_polls: chosen.avg_decode_polls,
        cache_live: chosen.cache_live,
        cache_session: chosen.cache_session,
        cache_session_cached: chosen.cache_session_cached,
        cache_session_total: chosen.cache_session_total,
        spec_accept: chosen.spec_accept,
        spec_accepted: chosen.spec_accepted,
        spec_drafted: chosen.spec_drafted,
        ttft_last: chosen.ttft_last,
        ttft_avg: chosen.ttft_avg,
        ttft_n: chosen.ttft_n,
        active_slots: chosen.active_slots.clone(),
        ctx_used: chosen.ctx_used,
        ctx_max: chosen.ctx_max,
        requests_processing: chosen.requests_processing,
        requests_deferred: chosen.requests_deferred,
        history,
    })
}


