//! Prefill / decode statistics (rewritten version, simple).
//!
//! Principles:
//! - live prefill/decode t/s is a sliding-window rate (per-slot `/slots`
//!   snapshots over 10 s), summed over active slots; the last value is
//!   held while a slot is still in phase (no `--` flicker), `None`
//!   only when truly idle/stale;
//! - averages are computed over reads (polls), not over
//!   requests: each poll with an observed speed feeds the average;
//! - the per-request history is stored on the backend side (memory only,
//!   last 50) and returned on every poll: the frontend only
//!   displays it, no accumulation/dedupe on the React side;
//! - no disk persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::AppHandle;

use crate::locale::{lang_of, msg, Msg};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptMetrics {
    pub prompt_tps: Option<f64>,
    pub predicted_tps: Option<f64>,
    pub requests_processing: Option<u64>,
    pub requests_deferred: Option<u64>,
    pub tokens_cache_used: Option<u64>,
    pub raw_scrape_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingSample {
    pub at: String,
    pub prompt_n: Option<u64>,
    pub prompt_ms: Option<f64>,
    pub prompt_tps: Option<f64>,
    /// Prompt KV-cache reuse of this request, as a coherent pair captured
    /// together: cached tokens + the prompt size they were counted
    /// against (slot freeze, `timings`, or same counter window — never
    /// mixed, the fork counts evaluated-only in `prompt_tokens_total`).
    pub cache_n: Option<u64>,
    pub cache_total_n: Option<u64>,
    pub predicted_n: Option<u64>,
    pub predicted_ms: Option<f64>,
    pub predicted_tps: Option<f64>,
    /// Speculative decoding of this request (drafted / accepted).
    pub spec_drafted: Option<u64>,
    pub spec_accepted: Option<u64>,
}

#[derive(Default)]
pub struct StatsState {
    /// Averages over polls (memory only).
    pub avg: Mutex<PollAvg>,
    /// Per-request history (memory only, last 50).
    pub hist: Mutex<Vec<TimingSample>>,
    /// Slot tracking between two polls.
    pub track: Mutex<SimpleTrack>,
}

/// Sums for averages over reads (polls).
#[derive(Default, Clone, Copy)]
pub struct PollAvg {
    pub sum_pre: f64,
    pub n_pre: u64,
    pub sum_dec: f64,
    pub n_dec: u64,
}

#[derive(Default)]
pub struct SimpleTrack {
    pub prev: HashMap<u32, PrevSlot>,
    /// Cached n_ctx (stable for the server's lifetime).
    pub ctx_max: Option<u64>,
    /// Counters at the last poll without activity: exact baseline for a
    /// request's window (covers a prefill faster than the polling).
    pub base: Option<Ctr>,
    /// Last closed request: its flushes may arrive in the
    /// following polls (counters flushed after our /metrics read).
    pub pending: Option<Pending>,
    /// Live cache ratio frozen at prefill end: decoding resets the
    /// slot counters, so the frozen value is shown while decoding
    /// (cleared when idle).
    pub cache_held: Option<f64>,
}

#[derive(Clone)]
pub(crate) struct Pending {
    start: Option<Ctr>,
    wall: TimingSample,
    left: u8,
    pre_ok: bool,
    dec_ok: bool,
    cache_ok: bool,
    spec_ok: bool,
}

impl Pending {
    /// Everything counter-resolvable arrived: no more replay needed.
    pub(crate) fn resolved(&self) -> bool {
        self.pre_ok && self.dec_ok && self.cache_ok && self.spec_ok
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Ctr {
    pt: f64,
    ps: f64,
    dt: f64,
    ds: f64,
    ct: f64,
    sd: f64,
    sa: f64,
}

fn read_ctrs(mtext: &str) -> Ctr {
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
pub(crate) fn cache_pair(start: Option<Ctr>, end: Ctr) -> (Option<u64>, Option<u64>) {
    let Some(st) = start else {
        return (None, None);
    };
    let dpt = end.pt - st.pt;
    let dct = (end.ct - st.ct).max(0.0);
    if dpt > 0.0 {
        (Some(dct as u64), Some((dpt + dct) as u64))
    } else {
        (None, None)
    }
}

/// Session cache reuse from finished requests: (cached, prompt-size)
/// sums over samples carrying a coherent pair (others skipped, never
/// mixed). Always consistent with the displayed per-request ratios.
pub(crate) fn session_cache(history: &[TimingSample]) -> (u64, u64) {
    let mut cached = 0u64;
    let mut total = 0u64;
    for s in history {
        if let (Some(c), Some(t)) = (s.cache_n, s.cache_total_n) {
            if t > 0 {
                cached += c.min(t);
                total += t;
            }
        }
    }
    (cached, total)
}

#[derive(Clone)]
pub(crate) struct PrevSlot {
    task: Option<u64>,
    processing: bool,
    prompt_total: u64,
    processed: u64,
    decoded: u64,
    first_seen: std::time::Instant,
    /// Prefill -> decode switch instant (None if never observed).
    prefill_end: Option<std::time::Instant>,
    /// /metrics counters when the request appeared (exact window).
    start: Option<Ctr>,
    /// True if another request was running in parallel (shared window).
    shared: bool,
    /// Recent (poll time, processed, decoded) snapshots for the
    /// sliding-window live rate (pruned to `WIN_SECS`).
    snaps: Vec<(std::time::Instant, u64, u64)>,
    /// Last windowed rate per phase, held while in phase so the display
    /// doesn't flicker `--`. `None` when idle or stale.
    smooth_pre: Option<f64>,
    smooth_dec: Option<f64>,
    /// Last poll with observed progress, per phase (staleness timeout).
    last_pre_at: Option<std::time::Instant>,
    last_dec_at: Option<std::time::Instant>,
    /// Cache counts frozen at the last prefill poll (same instant, same
    /// source — the only coherent pair): cached prefix + prompt size.
    /// Decoding resets the slot counters, so these (not live values)
    /// feed the frozen display and the request history.
    cache_final_n: Option<u64>,
    cache_final_total: Option<u64>,
}

/// Sliding window for the live rate: wide enough to cover several
/// prefill batches (`/slots` counters advance in coarse jumps, with
/// multi-second gaps between polls that see progress).
pub(crate) const WIN_SECS: f64 = 10.0;
/// Minimum span to compute a rate (avoids div-by-~0 on task start).
pub(crate) const MIN_SPAN_SECS: f64 = 1.0;
/// No progress for this long while still marked processing: the held
/// value is dropped back to `None` (`--` display).
pub(crate) const STALE_SECS: f64 = 15.0;

/// Live rate over the sliding window: (newest - oldest)/span on
/// oldest-first `(age_secs, counter)` snapshots pruned to `WIN_SECS`.
/// `None` when the span is too short or nothing advanced (the caller
/// holds the previous value while the slot is still in phase).
/// Pure for testability.
pub(crate) fn window_rate(snaps: &[(f64, u64)]) -> Option<f64> {
    let &(a0, v0) = snaps.first()?;
    let &(a1, v1) = snaps.last()?;
    let span = a0 - a1; // oldest age minus newest age
    if span >= MIN_SPAN_SECS && v1 > v0 {
        Some((v1 - v0) as f64 / span)
    } else {
        None
    }
}

/// Keep the previous rate while the slot is still in phase and its
/// last progress is recent; `None` once stale (or never established).
pub(crate) fn hold_or_stale(
    fresh: Option<f64>,
    prev: Option<f64>,
    last_at: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<f64> {
    match fresh {
        Some(r) => Some(r),
        None => match (prev, last_at) {
            (Some(p), Some(t)) if now.duration_since(t).as_secs_f64() <= STALE_SECS => Some(p),
            _ => None,
        },
    }
}

/// Prefill is over for this task as soon as generation is observed
/// (**delta** of decoded tokens: absolute values may be non-zero for
/// other reasons, and staged prefills hit `processed == prompt_total`
/// mid-request). Latched for the task. Forces the decode
/// classification so prefill metrics freeze (live `--`, average kept)
/// instead of decaying while tokens generate. `fresh` tasks never
/// count as over. Pure for testability.
pub(crate) fn prefill_over(fresh: bool, prev_end: bool, dec_delta: u64) -> bool {
    !fresh && (prev_end || dec_delta > 0)
}

fn parse_metric(text: &str, name: &str) -> Option<f64> {
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

fn parse_metric_u64(text: &str, name: &str) -> Option<u64> {
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

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Push to history, normalizing (ms to 2 decimals, like the log) and
/// skipping duplicates: a test request is seen twice (its direct
/// `timings` + its slot tracking), with the same numbers.
fn push_history(state: &StatsState, mut s: TimingSample) {
    if let Some(ms) = s.prompt_ms {
        s.prompt_ms = Some(r2(ms));
    }
    if let Some(ms) = s.predicted_ms {
        s.predicted_ms = Some(r2(ms));
    }
    if let Ok(mut h) = state.hist.lock() {
        // Dedupe on speeds only: cache/spec ride along on whichever
        // sighting wins (counter-window vs exact `timings` values may
        // differ slightly with flush delays — keying on them would
        // double-list test requests).
        let dup = h.iter().rev().take(3).any(|e| {
            e.prompt_n == s.prompt_n
                && e.prompt_ms == s.prompt_ms
                && e.predicted_n == s.predicted_n
                && e.predicted_ms == s.predicted_ms
        });
        if !dup {
            h.push(s);
            if h.len() > 50 {
                let drop = h.len() - 50;
                h.drain(0..drop);
            }
        }
    }
}

/// End-of-request sample from the observed instants:
/// prefill = [first_seen, prefill_end], decode = [prefill_end, close].
/// Each speed only covers its own phase (no dilution).
fn close_sample(p: &PrevSlot, close: std::time::Instant, cur_processed: u64, cur_decoded: u64) -> TimingSample {
    let pn = p.prompt_total.max(cur_processed).max(p.processed);
    let dn = p.decoded.max(cur_decoded);
    let (prompt_ms, prompt_tps, predicted_ms, predicted_tps) = match p.prefill_end {
        Some(t) => {
            let pre_sec = t.duration_since(p.first_seen).as_secs_f64();
            let dec_sec = close.duration_since(t).as_secs_f64();
            let pre = if pre_sec > 0.001 && pn > 0 {
                let r = pn as f64 / pre_sec;
                (Some(pre_sec * 1000.0), Some(r))
            } else {
                (None, None)
            };
            let dec = if dec_sec > 0.001 && dn > 0 {
                let r = dn as f64 / dec_sec;
                (Some(dec_sec * 1000.0), Some(r))
            } else {
                (None, None)
            };
            (pre.0, pre.1, dec.0, dec.1)
        }
        // Never seen switching: all-prefill (nothing generated) or all-decode.
        None => {
            let total_ms = close.duration_since(p.first_seen).as_secs_f64() * 1000.0;
            if dn > 0 {
                let r = if total_ms > 1.0 {
                    Some(dn as f64 / (total_ms / 1000.0))
                } else {
                    None
                };
                (None, None, if r.is_some() { Some(total_ms) } else { None }, r)
            } else if total_ms > 1.0 && pn > 0 {
                let r = pn as f64 / (total_ms / 1000.0);
                (Some(total_ms), Some(r), None, None)
            } else {
                (None, None, None, None)
            }
        }
    };
    TimingSample {
        at: chrono::Local::now().to_rfc3339(),
        prompt_n: Some(pn),
        prompt_ms,
        prompt_tps,
        cache_n: None, // filled by `with_counters` / `finish_sample`
        cache_total_n: None,
        predicted_n: Some(dn),
        predicted_ms,
        predicted_tps,
        spec_drafted: None, // filled by `with_counters`
        spec_accepted: None,
    }
}

fn has_speeds(s: &TimingSample) -> bool {
    s.prompt_tps.is_some() || s.predicted_tps.is_some()
}

/// Exact speeds of a task from the server log, written at the end of
/// each request (`slot print_timing ... task <id> ...`):
/// (prompt_n, prompt_ms, predicted_n, predicted_ms). The log is the only
/// exact and immediate source (/metrics counters flush late).
fn lookup_log_timings(log_path: &std::path::Path, task: u64) -> Option<(u64, f64, u64, f64)> {
    let bytes = std::fs::read(log_path).ok()?;
    let tail = if bytes.len() > 262144 {
        &bytes[bytes.len() - 262144..]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(tail);
    let key = format!("task {task} |");
    let mut prompt: Option<(u64, f64)> = None;
    let mut gen: Option<(u64, f64)> = None;
    for line in text.lines() {
        if !line.contains("print_timing") || !line.contains(&key) {
            continue;
        }
        // `... prompt eval time =     139.31 ms /     4 tokens`
        // `...        eval time =     933.36 ms /    30 tokens`
        if let Some(eq) = line.find('=') {
            if let Some((ms, n)) = parse_ms_n(&line[eq + 1..]) {
                if line.contains("prompt eval time") {
                    prompt = Some((n, ms));
                } else if line.contains("eval time") {
                    gen = Some((n, ms));
                }
            }
        }
    }
    match (prompt, gen) {
        (Some((pn, pms)), Some((dn, dms))) => Some((pn, pms, dn, dms)),
        _ => None,
    }
}

/// `139.31 ms / 4 tokens` -> (ms, n).
fn parse_ms_n(right: &str) -> Option<(f64, u64)> {
    let mut parts = right.split('/');
    let ms = parts
        .next()?
        .trim()
        .strip_suffix("ms")?
        .trim()
        .parse::<f64>()
        .ok()?;
    let n = parts.next()?.split_whitespace().next()?.parse::<u64>().ok()?;
    if ms > 0.0 {
        Some((ms, n))
    } else {
        None
    }
}

/// End-of-request sample, by order of reliability:
/// 1. server log (exact, immediate), 2. /metrics counters over the window,
/// 3. wall clock observed via /slots. Also returns the counter-sourced
/// or log-sourced phases (for catch-up).
#[allow(clippy::too_many_arguments)]
fn finish_sample(
    p: &PrevSlot,
    close: std::time::Instant,
    cur_processed: u64,
    cur_decoded: u64,
    log_path: Option<&std::path::Path>,
    win_start: Option<Ctr>,
    ctr_now: Ctr,
) -> (TimingSample, bool, bool) {
    let mut s = close_sample(p, close, cur_processed, cur_decoded);
    let mut pre_ok = false;
    let mut dec_ok = false;
    if let (Some(tid), Some(lp)) = (p.task, log_path) {
        if let Some((pn, pms, dn, dms)) = lookup_log_timings(lp, tid) {
            s.prompt_n = Some(pn);
            s.prompt_ms = Some(pms);
            s.prompt_tps = Some(pn as f64 / (pms / 1000.0));
            s.predicted_n = Some(dn);
            s.predicted_ms = Some(dms);
            s.predicted_tps = Some(dn as f64 / (dms / 1000.0));
            pre_ok = true;
            dec_ok = true;
        }
    }
    if !(pre_ok && dec_ok) {
        let (s2, po, already) = with_counters(s, win_start, ctr_now);
        s = s2;
        pre_ok = pre_ok || po;
        dec_ok = dec_ok || already;
    }
    // Cache counts over the same window, whatever the speed source
    // (the server log carries no cache info). The frozen slot pair
    // wins: captured together at prefill end, immune to the
    // decode-time counter reset and to aggregate-counter semantics.
    if s.cache_n.is_none() {
        s.cache_n = p.cache_final_n;
        s.cache_total_n = p.cache_final_total;
    }
    if s.cache_n.is_none() {
        let (c, t) = cache_pair(win_start, ctr_now);
        s.cache_n = c;
        s.cache_total_n = t;
    }
    (s, pre_ok, dec_ok)
}

/// Cover the sample with the /metrics counters over the
/// [appearance, end] window: off-cache tokens and server-measured seconds,
/// i.e. exactly the semantics of the log's `print_timing`. Wall-clock
/// fallback (close_sample) if the counters did not advance (server reset,
/// concurrent requests on the same slot, missing metrics).
fn with_counters(mut s: TimingSample, start: Option<Ctr>, end: Ctr) -> (TimingSample, bool, bool) {
    let mut pre_ok = false;
    let mut dec_ok = false;
    if let Some(st) = start {
        let dpt = end.pt - st.pt;
        let dps = end.ps - st.ps;
        if dpt > 0.0 && dps > 0.0005 && dpt >= 0.0 && dps >= 0.0 {
            s.prompt_n = Some(dpt as u64);
            s.prompt_ms = Some(dps * 1000.0);
            s.prompt_tps = Some(dpt / dps);
            pre_ok = true;
        }
        let ddt = end.dt - st.dt;
        let dds = end.ds - st.ds;
        if ddt > 0.0 && dds > 0.0005 && ddt >= 0.0 && dds >= 0.0 {
            s.predicted_n = Some(ddt as u64);
            s.predicted_ms = Some(dds * 1000.0);
            s.predicted_tps = Some(ddt / dds);
            dec_ok = true;
        }
        // Same-window coherent pair (see `cache_pair`).
        if s.cache_n.is_none() {
            let (c, t) = cache_pair(Some(st), end);
            s.cache_n = c;
            s.cache_total_n = t;
        }
        // Per-request speculative work over the same window (late
        // counter flushes are replayed by the pending catch-up).
        if s.spec_drafted.is_none() {
            let dd = end.sd - st.sd;
            let da = end.sa - st.sa;
            if dd > 0.0 && da >= 0.0 {
                s.spec_drafted = Some(dd as u64);
                s.spec_accepted = Some(da.max(0.0) as u64);
            }
        }
    }
    (s, pre_ok, dec_ok)
}

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

#[derive(Debug, Clone, serde::Serialize)]
pub struct SlotLive {
    pub id: u32,
    pub phase: String, // "prefill" | "decode"
    pub prompt_done: u64,
    pub prompt_total: u64,
    pub decoded: u64,
    pub live_tps: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Activity {
    /// Speeds over the interval (sum of all active slots). None when idle.
    pub prefill_tps: Option<f64>,
    pub decode_tps: Option<f64>,
    /// Averages over reads (polls) with speeds. Kept displayed.
    pub avg_prefill_tps: Option<f64>,
    pub avg_decode_tps: Option<f64>,
    pub avg_prefill_polls: u64,
    pub avg_decode_polls: u64,
    /// Prompt KV-cache reuse of the active request(s) (0..1).
    /// None when idle. Frozen while decoding (slot counters reset).
    pub cache_live: Option<f64>,
    /// Prompt KV-cache reuse over finished requests (history sums).
    pub cache_session: Option<f64>,
    pub cache_session_cached: u64,
    pub cache_session_total: u64,
    /// Speculative-decoding acceptance since server start (counters).
    /// None when no draft token was scored yet.
    pub spec_accept: Option<f64>,
    pub spec_accepted: u64,
    pub spec_drafted: u64,
    /// Time to first token: last finished request + history average (s).
    pub ttft_last: Option<f64>,
    pub ttft_avg: Option<f64>,
    pub ttft_n: u64,
    /// Running slots + what they are doing.
    pub active_slots: Vec<SlotLive>,
    /// Context: tokens used (active slots) / n_ctx.
    pub ctx_used: u64,
    pub ctx_max: Option<u64>,
    pub requests_processing: Option<u64>,
    pub requests_deferred: Option<u64>,
    /// Per-request history (backend, memory only).
    pub history: Vec<TimingSample>,
}

fn slot_u64(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// One activity poll: /metrics (totals) + /slots (live + history).
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

    let mtext = client
        .get(format!("{base}/metrics"))
        .send()
        .await
        .map_err(|e| format!("/metrics: {e}"))?
        .error_for_status()
        .map_err(|e| format!("/metrics statut: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let slots_json: Option<serde_json::Value> =
        match client.get(format!("{base}/slots")).send().await {
            Ok(r) => r.json().await.ok(),
            Err(_) => None,
        };

    // --- Synchronous section under lock (no await below).
    // Current poll's /metrics counters (exact per-request windows).
    let ctr_now = read_ctrs(&mtext);
    // Server log (immediate ground truth per task). Missing if the server
    // was adopted with a different log file -> counters/wall fallback.
    let log_path = crate::state::app_data_dir(&app)
        .map(|d| d.join("logs").join("llama-server.log"))
        .ok();
    let mut track = state.track.lock().map_err(|e| e.to_string())?;
    let now = std::time::Instant::now();

    let mut active_slots = vec![];
    let mut finished = vec![];
    let mut smooth_pre_sum = 0.0f64;
    let mut smooth_dec_sum = 0.0f64;
    let mut has_pre = false;
    let mut has_dec = false;
    let mut ctx_used = 0u64;
    // Prompt-sized cache accumulators (prefill-phase slots only:
    // decoding resets the slot counters).
    let mut cache_wsum = 0u64;
    let mut cache_w = 0u64;
    let mut saw_fresh = false;
    // Requests processed in parallel this poll (shared counter windows).
    let n_proc = slots_json
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|s| {
                    s.get("is_processing")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);

    if let Some(arr) = slots_json.as_ref().and_then(|v| v.as_array()) {
        // n_ctx: stable, anchor it on the first poll that sees it.
        // The KV cache is shared across slots: the max is n_ctx
        // (not n_ctx x slot count).
        if track.ctx_max.is_none() {
            for s in arr {
                let n = slot_u64(s, "n_ctx");
                if n > 0 {
                    track.ctx_max = Some(n);
                    break;
                }
            }
        }
        for s in arr {
            let id = slot_u64(s, "id") as u32;
            let processing = s
                .get("is_processing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let task = s.get("id_task").and_then(|v| v.as_u64());
            let prompt_total = slot_u64(s, "n_prompt_tokens");
            let processed = slot_u64(s, "n_prompt_tokens_processed");
            let slot_cache = slot_u64(s, "n_prompt_tokens_cache");
            let decoded = s
                .get("next_token")
                .and_then(|nt| nt.get(0))
                .and_then(|t| t.get("n_decoded"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let prev = track.prev.get(&id).cloned();
            // Context: tokens loaded in the slot, even when idle (the
            // conversational cache survives the end of the request).
            ctx_used += prompt_total + decoded;

            if processing {
                let fresh = prev.is_none()
                    || matches!((prev.as_ref().and_then(|p| p.task), task), (Some(a), Some(b)) if a != b);
                saw_fresh = saw_fresh || fresh;
                // Previous task preempted with no closing poll: close it
                // here, otherwise it is lost.
                if fresh {
                    // New task: hand over, but give the previous request's
                    // late counter flushes one last replay first (its spec
                    // attribution stays safe: the new task just appeared).
                    // Cache is NOT adopted (window would mix both tasks).
                    if let Some(pend) = track.pending.take() {
                        let (better, _, _) = with_counters(pend.wall.clone(), pend.start, ctr_now);
                        if better.spec_drafted.is_some() {
                            if let Ok(mut h) = state.hist.lock() {
                                if h.last().map(|l| l.at == pend.wall.at).unwrap_or(false) {
                                    let e = h.last_mut().unwrap();
                                    if e.spec_drafted.is_none() {
                                        e.spec_drafted = better.spec_drafted;
                                        e.spec_accepted = better.spec_accepted;
                                    }
                                }
                            }
                        }
                    }
                    track.pending = None;
                    if let Some(p) = prev.as_ref() {
                        if p.processing {
                            let (s, pre_ok, dec_ok) = finish_sample(
                                &p,
                                now,
                                p.processed,
                                p.decoded,
                                log_path.as_deref(),
                                p.start,
                                ctr_now,
                            );
                            if has_speeds(&s) {
                                finished.push(s.clone());
                                // Wide window: counters are flushed
                                // with a variable delay (several seconds).
                                let cache_ok = s.cache_n.is_some();
                                let spec_ok = s.spec_drafted.is_some();
                                track.pending = Some(Pending {
                                    start: p.start,
                                    wall: s,
                                    left: 30,
                                    pre_ok,
                                    dec_ok,
                                    cache_ok,
                                    spec_ok,
                                });
                            }
                        }
                    }
                }
                // Deltas vs the previous poll (0 on a fresh sighting): the
                // only reliable signals. NOTE: `processed >= prompt_total`
                // must NOT mark prefill end — this fork grows
                // `n_prompt_tokens` in stages during prefill, hitting
                // equality mid-request.
                let d_pre = processed.saturating_sub(prev.as_ref().map(|p| p.processed).unwrap_or(processed));
                let d_dec = decoded.saturating_sub(prev.as_ref().map(|p| p.decoded).unwrap_or(decoded));
                // Fresh task or prefill->decode switch: drop old snapshots
                // (previous task's — or the prefill's zeros' — speeds must
                // not leak into the new phase's window).
                let prev_end = prev.as_ref().and_then(|p| p.prefill_end).is_some();
                let switch_now = !fresh && !prev_end && d_dec > 0;
                let mut snaps = if fresh || switch_now {
                    Vec::new()
                } else {
                    prev.as_ref().map(|p| p.snaps.clone()).unwrap_or_default()
                };
                snaps.retain(|(t, _, _)| now.duration_since(*t).as_secs_f64() <= WIN_SECS);
                snaps.push((now, processed, decoded));
                let pre_av: Vec<(f64, u64)> = snaps
                    .iter()
                    .map(|(t, p, _)| (now.duration_since(*t).as_secs_f64(), *p))
                    .collect();
                let dec_av: Vec<(f64, u64)> = snaps
                    .iter()
                    .map(|(t, _, d)| (now.duration_since(*t).as_secs_f64(), *d))
                    .collect();
                // Fresh task: reset the held rates too.
                let (base_pre, base_dec, base_pre_at, base_dec_at) = if fresh {
                    (None, None, None, None)
                } else {
                    (
                        prev.as_ref().and_then(|p| p.smooth_pre),
                        prev.as_ref().and_then(|p| p.smooth_dec),
                        prev.as_ref().and_then(|p| p.last_pre_at),
                        prev.as_ref().and_then(|p| p.last_dec_at),
                    )
                };
                let last_pre_at = if d_pre > 0 { Some(now) } else { base_pre_at };
                let last_dec_at = if d_dec > 0 { Some(now) } else { base_dec_at };
                let smooth_pre = hold_or_stale(window_rate(&pre_av), base_pre, last_pre_at, now);
                let smooth_dec = hold_or_stale(window_rate(&dec_av), base_dec, last_dec_at, now);
                // Generation observed (delta) latches the task into
                // decode: prefill live goes `--` and its average freezes
                // instead of decaying while tokens generate.
                let generating = prefill_over(fresh, prev_end, d_dec);
                let phase = if generating {
                    "decode"
                } else if prompt_total > 0 && processed < prompt_total {
                    "prefill"
                } else if smooth_pre.is_some() {
                    // Staged-prefill boundary (`processed == prompt_total`
                    // with no generation yet): hold the prefill display
                    // instead of flickering `--`.
                    "prefill"
                } else {
                    "decode"
                };
                let live_tps = if phase == "prefill" { smooth_pre } else { smooth_dec };
                // Frozen cache counts: refreshed while prefilling, carried
                // over untouched once decoding (slot counters reset).
                // A fresh task never inherits the previous one.
                let (cache_final_n, cache_final_total) = if phase == "prefill" && prompt_total > 0 {
                    (Some(slot_cache.min(prompt_total)), Some(prompt_total))
                } else if fresh {
                    (None, None)
                } else {
                    (
                        prev.as_ref().and_then(|p| p.cache_final_n),
                        prev.as_ref().and_then(|p| p.cache_final_total),
                    )
                };
                if phase == "prefill" {
                    if let Some(v) = smooth_pre {
                        smooth_pre_sum += v;
                        has_pre = true;
                    }
                    // Prompt-sized-weighted reuse over prefill slots.
                    if let (Some(c), Some(t)) = (cache_final_n, cache_final_total) {
                        if t > 0 {
                            cache_wsum += c;
                            cache_w += t;
                        }
                    }
                } else if let Some(v) = smooth_dec {
                    smooth_dec_sum += v;
                    has_dec = true;
                }
                active_slots.push(SlotLive {
                    id,
                    phase: phase.to_string(),
                    prompt_done: processed,
                    prompt_total,
                    decoded,
                    live_tps,
                });
                let first = if fresh {
                    now
                } else {
                    prev.as_ref().map(|p| p.first_seen).unwrap_or(now)
                };
                // Counter window: anchor it at the task's appearance.
                let start = if fresh {
                    Some(ctr_now)
                } else {
                    prev.as_ref().and_then(|p| p.start)
                };
                // Prefill -> decode switch: generation observed (delta).
                // Neither absolute `decoded` (may be non-zero for other
                // reasons) nor `processed >= prompt_total` (staged
                // prefill hits equality mid-request) may set it.
                let prefill_end = match prev.as_ref().and_then(|p| p.prefill_end) {
                    Some(t) => Some(t),
                    None if d_dec > 0 => Some(now),
                    None => None,
                };
                track.prev.insert(
                    id,
                    PrevSlot {
                        task,
                        processing: true,
                        prompt_total,
                        processed,
                        decoded,
                        first_seen: first,
                        prefill_end,
                        start,
                        shared: n_proc > 1 || prev.as_ref().map(|p| p.shared).unwrap_or(false),
                        snaps,
                        smooth_pre,
                        smooth_dec,
                        last_pre_at,
                        last_dec_at,
                        cache_final_n,
                        cache_final_total,
                    },
                );
            } else {
                match prev {
                    // Transition active -> idle: finished request.
                    Some(p) if p.processing => {
                        // Exact window: baseline from the last idle poll (covers
                        // a prefill faster than the polling), except for
                        // parallel requests (keep the observed window).
                        let win_start = if p.shared { p.start } else { track.base.or(p.start) };
                        let (s, pre_ok, dec_ok) = finish_sample(
                            &p,
                            now,
                            processed,
                            decoded,
                            log_path.as_deref(),
                            win_start,
                            ctr_now,
                        );
                        if has_speeds(&s) {
                            finished.push(s.clone());
                            // Wide window: counters are flushed
                            // with a variable delay (several seconds).
                            let cache_ok = s.cache_n.is_some();
                            let spec_ok = s.spec_drafted.is_some();
                            track.pending = Some(Pending {
                                start: win_start,
                                wall: s,
                                left: 30,
                                pre_ok,
                                dec_ok,
                                cache_ok,
                                spec_ok,
                            });
                        }
                        track.prev.insert(
                            id,
                            PrevSlot {
                                task,
                                processing: false,
                                prompt_total,
                                processed,
                                decoded,
                                first_seen: p.first_seen,
                                prefill_end: None,
                                start: None,
                                shared: p.shared,
                                snaps: Vec::new(),
                                smooth_pre: None,
                                smooth_dec: None,
                                last_pre_at: None,
                                last_dec_at: None,
                                cache_final_n: None,
                                cache_final_total: None,
                            },
                        );
                    }
                    // Task finished between two polls (never seen active):
                    // no observable speeds, ignore it (no stats).
                    Some(p) => {
                        track.prev.insert(
                            id,
                            PrevSlot {
                                task,
                                processing: false,
                                prompt_total,
                                processed,
                                decoded,
                                first_seen: p.first_seen,
                                prefill_end: None,
                                start: None,
                                shared: p.shared,
                                snaps: Vec::new(),
                                smooth_pre: None,
                                smooth_dec: None,
                                last_pre_at: None,
                                last_dec_at: None,
                                cache_final_n: None,
                                cache_final_total: None,
                            },
                        );
                    }
                    // Never seen: silent anchoring.
                    None => {
                        track.prev.insert(
                            id,
                            PrevSlot {
                                task,
                                processing: false,
                                prompt_total,
                                processed,
                                decoded,
                                first_seen: now,
                                prefill_end: None,
                                start: None,
                                shared: false,
                                snaps: Vec::new(),
                                smooth_pre: None,
                                smooth_dec: None,
                                last_pre_at: None,
                                last_dec_at: None,
                                cache_final_n: None,
                                cache_final_total: None,
                            },
                        );
                    }
                }
            }
        }
    }
    // Baseline updated AFTER use (idle polls only).
    if active_slots.is_empty() {
        track.base = Some(ctr_now);
    }
    // Catch-up: counters may be flushed just after our closing poll's
    // /metrics read. Replay the window on the following idle polls
    // (safe attribution: nothing else is running). Stays alive until
    // speeds, cache AND spec are all resolved (bounded by `left`).
    let amend = if active_slots.is_empty() {
        track
            .pending
            .as_ref()
            .filter(|p| p.left > 0 && !p.resolved())
            .map(|p| (p.start, p.wall.clone()))
    } else {
        None
    };
    let ctx_max = track.ctx_max;
    // Live cache reuse: fresh prompt-weighted mean while prefilling;
    // frozen while decoding (slot counters reset); cleared when idle.
    // A fresh task never inherits the previous request's value.
    if saw_fresh {
        track.cache_held = None;
    }
    let cache_live = if cache_w > 0 {
        let v = (cache_wsum as f64 / cache_w as f64).clamp(0.0, 1.0);
        track.cache_held = Some(v);
        Some(v)
    } else if !active_slots.is_empty() {
        track.cache_held
    } else {
        track.cache_held = None;
        None
    };
    drop(track);

    // Live: sliding-window rates, summed over active slots.
    // `None` when idle (no slot in phase with a fresh value) -> `--`.
    let prefill_tps = has_pre.then_some(smooth_pre_sum);
    let decode_tps = has_dec.then_some(smooth_dec_sum);
    // Speculative acceptance, session-wide (counters).
    let spec_accepted =
        parse_metric(&mtext, "llamacpp:spec_decode_num_accepted_tokens_total").unwrap_or(0.0).max(0.0)
            as u64;
    let spec_drafted =
        parse_metric(&mtext, "llamacpp:spec_decode_num_draft_tokens_total").unwrap_or(0.0).max(0.0)
            as u64;
    let spec_accept = if spec_drafted > 0 {
        Some((spec_accepted as f64 / spec_drafted as f64).clamp(0.0, 1.0))
    } else {
        None
    };

    // Averages over reads: each poll with a speed counts as 1.
    let (avg_pre, avg_dec, n_pre, n_dec) = match state.avg.lock() {
        Ok(mut a) => {
            if let Some(v) = prefill_tps {
                a.sum_pre += v;
                a.n_pre += 1;
            }
            if let Some(v) = decode_tps {
                a.sum_dec += v;
                a.n_dec += 1;
            }
            let ap = if a.n_pre > 0 {
                Some(a.sum_pre / a.n_pre as f64)
            } else {
                None
            };
            let ad = if a.n_dec > 0 {
                Some(a.sum_dec / a.n_dec as f64)
            } else {
                None
            };
            (ap, ad, a.n_pre, a.n_dec)
        }
        Err(_) => (None, None, 0, 0),
    };

    for s in finished {
        push_history(&state, s);
    }
    // Apply the catch-up to the last line (same `at`).
    // Stop as soon as everything counter-resolvable arrived.
    if let Some((st, wall)) = amend {
        let (better, pre_now, dec_now) = with_counters(wall.clone(), st, ctr_now);
        let cache_now = better.cache_n.is_some();
        let spec_now = better.spec_drafted.is_some();
        if has_speeds(&better) {
            if let Ok(mut h) = state.hist.lock() {
                if h.last().map(|l| l.at == wall.at).unwrap_or(false) {
                    *h.last_mut().unwrap() = better;
                }
            }
        }
        if let Ok(mut t) = state.track.lock() {
            if let Some(p) = t.pending.as_mut() {
                p.pre_ok = p.pre_ok || pre_now;
                p.dec_ok = p.dec_ok || dec_now;
                p.cache_ok = p.cache_ok || cache_now;
                p.spec_ok = p.spec_ok || spec_now;
                if p.left > 0 {
                    p.left -= 1;
                }
                if p.left == 0 || p.resolved() {
                    t.pending = None;
                }
            }
        }
    }
    let history = state
        .hist
        .lock()
        .map(|h| h.clone())
        .unwrap_or_default();
    // Session cache reuse, summed over finished requests (immune to
    // aggregate-counter semantics and flush delays).
    let (sess_cached, sess_total) = session_cache(&history);
    let cache_session = if sess_total > 0 {
        Some(sess_cached as f64 / sess_total as f64)
    } else {
        None
    };
    // Time to first token from finished requests (prompt phase time).
    let mut ttft_sum = 0.0f64;
    let mut ttft_n = 0u64;
    let mut ttft_last: Option<f64> = None;
    for s in &history {
        if let Some(ms) = s.prompt_ms {
            if ms > 0.0 {
                ttft_sum += ms / 1000.0;
                ttft_n += 1;
                ttft_last = Some(ms / 1000.0);
            }
        }
    }
    let ttft_avg = if ttft_n > 0 {
        Some(ttft_sum / ttft_n as f64)
    } else {
        None
    };

    Ok(Activity {
        prefill_tps,
        decode_tps,
        avg_prefill_tps: avg_pre,
        avg_decode_tps: avg_dec,
        avg_prefill_polls: n_pre,
        avg_decode_polls: n_dec,
        cache_live,
        cache_session,
        cache_session_cached: sess_cached,
        cache_session_total: sess_total,
        spec_accept,
        spec_accepted,
        spec_drafted,
        ttft_last,
        ttft_avg,
        ttft_n,
        active_slots,
        ctx_used,
        ctx_max,
        requests_processing: parse_metric_u64(&mtext, "llamacpp:requests_processing"),
        requests_deferred: parse_metric_u64(&mtext, "llamacpp:requests_deferred"),
        history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(prompt_n: u64, prompt_ms: f64, predicted_n: u64, predicted_ms: f64) -> TimingSample {
        TimingSample {
            at: "2026-01-01T00:00:00+00:00".to_string(),
            prompt_n: Some(prompt_n),
            prompt_ms: Some(prompt_ms),
            prompt_tps: Some(prompt_n as f64 / (prompt_ms / 1000.0)),
            cache_n: None,
            cache_total_n: None,
            predicted_n: Some(predicted_n),
            predicted_ms: Some(predicted_ms),
            predicted_tps: Some(predicted_n as f64 / (predicted_ms / 1000.0)),
            spec_drafted: None,
            spec_accepted: None,
        }
    }

    #[test]
    fn session_cache_sums_coherent_pairs() {
        // The user's case: req1 fresh (338 prompt, 0 cached), req2 with
        // 880 cached of 894 — evaluated-only counters must not leak in.
        let mut a = sample(338, 1000.0, 542, 15000.0);
        a.cache_n = Some(0);
        a.cache_total_n = Some(338);
        let mut b = sample(14, 50.0, 100, 3000.0);
        b.cache_n = Some(880);
        b.cache_total_n = Some(894);
        let c = sample(200, 500.0, 2, 60.0); // unknown: skipped
        let (cached, total) = session_cache(&[a, b, c]);
        assert_eq!((cached, total), (880, 1232));
        assert!(session_cache(&[]).1 == 0);
    }

    #[test]
    fn spec_counters_parse_beside_per_pos_lines() {
        // Real /metrics adjacency: per-position lines must not shadow
        // the plain totals.
        let text = "llamacpp:spec_decode_num_accepted_tokens_per_pos_total{position=\"0\"} 500\nllamacpp:spec_decode_num_accepted_tokens_per_pos_total{position=\"1\"} 437\nllamacpp:spec_decode_num_accepted_tokens_total 937\nllamacpp:spec_decode_num_draft_tokens_total 1993\nllamacpp:spec_decode_num_drafts_total 100\n";
        assert_eq!(
            parse_metric(text, "llamacpp:spec_decode_num_accepted_tokens_total"),
            Some(937.0)
        );
        assert_eq!(
            parse_metric(text, "llamacpp:spec_decode_num_draft_tokens_total"),
            Some(1993.0)
        );
    }

    #[test]
    fn counters_cover_spec_window() {
        let s = sample(1, 10.0, 1, 10.0);
        let start = Ctr { pt: 1.0, ps: 1.0, dt: 1.0, ds: 1.0, ct: 0.0, sd: 100.0, sa: 40.0 };
        let end = Ctr { pt: 2.0, ps: 2.0, dt: 2.0, ds: 2.0, ct: 0.0, sd: 130.0, sa: 55.0 };
        let (out, _, _) = with_counters(s, Some(start), end);
        assert_eq!(out.spec_drafted, Some(30));
        assert_eq!(out.spec_accepted, Some(15));
        // No draft in the window: no spec info (not zeros).
        let (out2, _, _) = with_counters(sample(1, 10.0, 1, 10.0), Some(end), end);
        assert_eq!(out2.spec_drafted, None);
    }

    #[test]
    fn cache_pair_keeps_window_coherent() {
        // The fork counts evaluated-only in prompt_tokens_total: the
        // prompt size is evaluated + cached (user's 14 + 880 case).
        let start = Ctr { pt: 100.0, ps: 0.0, dt: 0.0, ds: 0.0, ct: 20.0, sd: 0.0, sa: 0.0 };
        let end = Ctr { pt: 114.0, ps: 0.0, dt: 0.0, ds: 0.0, ct: 900.0, sd: 0.0, sa: 0.0 };
        assert_eq!(cache_pair(Some(start), end), (Some(880), Some(894)));
        // No evaluated tokens in the window: no pair (not zeros).
        assert_eq!(cache_pair(Some(start), start), (None, None));
        assert_eq!(cache_pair(None, end), (None, None));
    }

    #[test]
    fn counters_cover_cache_too() {
        let s = sample(1, 10.0, 1, 10.0);
        let start = Ctr { pt: 100.0, ps: 5.0, dt: 50.0, ds: 10.0, ct: 10.0, sd: 0.0, sa: 0.0 };
        let end = Ctr { pt: 120.0, ps: 6.0, dt: 80.0, ds: 12.0, ct: 20.0, sd: 0.0, sa: 0.0 };
        let (out, _, _) = with_counters(s, Some(start), end);
        assert_eq!((out.cache_n, out.cache_total_n), (Some(10), Some(30)));
    }

    #[test]
    fn metric_parser_ignores_comments_and_labels() {
        let text = "# HELP llamacpp:prompt_tokens_total foo\nllamacpp:prompt_tokens_total 12\nllamacpp:prompt_tokens_total{le=\"1\"} 99\n";
        assert_eq!(parse_metric(text, "llamacpp:prompt_tokens_total"), Some(12.0));
        assert_eq!(parse_metric(text, "llamacpp:missing"), None);
    }

    #[test]
    fn metric_parser_scans_past_non_matching_lines() {
        // Regression: an early `?` aborted the scan at the first
        // mismatch, so only a first-placed metric ever parsed.
        let text = "# TYPE x gauge\nllamacpp:other_metric 5\nllamacpp:prompt_tokens_total 42\n";
        assert_eq!(parse_metric(text, "llamacpp:prompt_tokens_total"), Some(42.0));
        assert_eq!(parse_metric(text, "llamacpp:other_metric"), Some(5.0));
        assert_eq!(parse_metric(text, "llamacpp:absent"), None);
    }

    #[test]
    fn ms_n_parser_reads_log_format() {
        assert_eq!(parse_ms_n("     139.31 ms /     4 tokens"), Some((139.31, 4)));
        assert_eq!(parse_ms_n("0 ms / 0 tokens"), None);
        assert_eq!(parse_ms_n("nope"), None);
    }

    #[test]
    fn counters_cover_exact_window() {
        let s = sample(1, 10.0, 1, 10.0);
        let start = Ctr { pt: 100.0, ps: 5.0, dt: 50.0, ds: 10.0, ct: 20.0, sd: 0.0, sa: 0.0 };
        let end = Ctr { pt: 110.0, ps: 6.0, dt: 80.0, ds: 12.0, ct: 26.0, sd: 0.0, sa: 0.0 };
        let (out, pre_ok, dec_ok) = with_counters(s, Some(start), end);
        assert!(pre_ok && dec_ok);
        assert_eq!(out.prompt_n, Some(10));
        assert_eq!(out.predicted_n, Some(30));
        assert!((out.prompt_tps.unwrap() - 10.0).abs() < 1e-9);
        assert!((out.predicted_tps.unwrap() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn counters_stall_keeps_wall_clock() {
        let s = sample(4, 100.0, 8, 200.0);
        let c = Ctr { pt: 1.0, ps: 1.0, dt: 1.0, ds: 1.0, ct: 0.0, sd: 0.0, sa: 0.0 };
        let (out, pre_ok, dec_ok) = with_counters(s.clone(), Some(c), c);
        assert!(!pre_ok && !dec_ok);
        assert_eq!(out.prompt_n, s.prompt_n);
    }

    #[test]
    fn close_sample_splits_phases() {
        let now = std::time::Instant::now();
        let p = PrevSlot {
            task: Some(7),
            processing: true,
            prompt_total: 10,
            processed: 10,
            decoded: 20,
            first_seen: now - std::time::Duration::from_secs(2),
            prefill_end: Some(now - std::time::Duration::from_secs(1)),
            start: None,
            shared: false,
            snaps: Vec::new(),
            smooth_pre: None,
            smooth_dec: None,
            last_pre_at: None,
            last_dec_at: None,
            cache_final_n: None,
            cache_final_total: None,
        };
        let s = close_sample(&p, now, 10, 20);
        // prefill: 10 tok / 1 s, decode: 20 tok / 1 s.
        assert!((s.prompt_tps.unwrap() - 10.0).abs() < 0.5);
        assert!((s.predicted_tps.unwrap() - 20.0).abs() < 0.5);
    }

    #[test]
    fn window_rate_covers_coarse_batch_jumps() {
        // True 400 t/s advancing in +2000 jumps every 5 s (0.5 s polls):
        // per-window deltas would flicker `--` / 4000, the 10 s windowed
        // rate stays coherent.
        let mut snaps: Vec<(f64, u64)> = vec![];
        let mut v = 0u64;
        // Oldest-first ages: 10 s ago .. now, +2000 at t=5 s and t=10 s.
        for i in 0..=20 {
            let age = 10.0 - i as f64 * 0.5;
            if i == 10 || i == 20 {
                v += 2000;
            }
            snaps.push((age, v));
        }
        let r = window_rate(&snaps).expect("window must cover both jumps");
        assert!((r - 400.0).abs() < 40.0, "windowed rate ≈ true rate, got {r}");
        // Span too short (task start): no rate yet, `--` display.
        assert_eq!(window_rate(&[(0.3, 100), (0.0, 150)]), None);
        // Flat counter: no rate (caller holds the previous value).
        assert_eq!(window_rate(&[(5.0, 100), (0.0, 100)]), None);
        assert_eq!(window_rate(&[]), None);
    }

    #[test]
    fn hold_keeps_rate_while_in_phase_then_drops() {
        let now = std::time::Instant::now();
        let fresh = now - std::time::Duration::from_secs_f64(0.5);
        // No new windowed rate but recent progress: hold.
        assert_eq!(hold_or_stale(None, Some(380.0), Some(fresh), now), Some(380.0));
        // Stale progress: back to `None` (`--` display).
        let old = now - std::time::Duration::from_secs_f64(STALE_SECS + 1.0);
        assert_eq!(hold_or_stale(None, Some(380.0), Some(old), now), None);
        assert_eq!(hold_or_stale(None, None, None, now), None);
        // Fresh windowed rate always wins.
        assert_eq!(hold_or_stale(Some(410.0), Some(380.0), Some(old), now), Some(410.0));
    }

    #[test]
    fn pending_lives_until_cache_and_spec_resolve() {
        // Speeds alone must NOT retire the catch-up: late cache/spec
        // flushes still need replay (the observed `--` in history).
        let p = Pending {
            start: None,
            wall: sample(10, 100.0, 5, 50.0),
            left: 30,
            pre_ok: true,
            dec_ok: true,
            cache_ok: false,
            spec_ok: false,
        };
        assert!(!p.resolved());
        let q = Pending { cache_ok: true, spec_ok: true, ..p };
        assert!(q.resolved());
    }

    #[test]
    fn prefill_end_latches_task_into_decode() {
        // Mid-prefill, nothing generated: still prefill.
        assert!(!prefill_over(false, false, 0));
        // Generation delta observed: over (this is what freezes prefill).
        assert!(prefill_over(false, false, 3));
        // Latched: stays over even if counters stall a poll.
        assert!(prefill_over(false, true, 0));
        // Fresh task: never over (a new prefill must always display).
        assert!(!prefill_over(true, true, 50));
        assert!(!prefill_over(true, false, 0));
    }

    #[test]
    fn history_drops_duplicates() {
        let state = StatsState::default();
        push_history(&state, sample(4, 139.31, 30, 933.36));
        push_history(&state, sample(4, 139.31, 30, 933.36));
        assert_eq!(state.hist.lock().unwrap().len(), 1);
        push_history(&state, sample(4, 139.31, 31, 933.36));
        assert_eq!(state.hist.lock().unwrap().len(), 2);
    }
}
