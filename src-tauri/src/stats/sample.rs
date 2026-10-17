//! Building one finished-request sample out of the observed window,
//! the server log timings, and the counter window.

use super::log::lookup_log_timings;
use super::types::{Ctr, PrevSlot, StatsState, TimingSample};

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


fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Push to history, normalizing (ms to 2 decimals, like the log) and
/// skipping duplicates: a test request is seen twice (its direct
/// `timings` + its slot tracking), with the same numbers.
pub(crate) fn push_history(state: &StatsState, mut s: TimingSample) {
    if let Ok(mut h) = state.hist.lock() {
        push_history_into(&mut h, &mut s);
    }
}

/// Same, on an already-locked history (used by the per-model poll loop,
/// which holds the lock for its whole synchronous section).
pub(crate) fn push_history_into(h: &mut Vec<TimingSample>, s: &mut TimingSample) {
    if let Some(ms) = s.prompt_ms {
        s.prompt_ms = Some(r2(ms));
    }
    if let Some(ms) = s.predicted_ms {
        s.predicted_ms = Some(r2(ms));
    }
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
        h.push(s.clone());
        if h.len() > 50 {
            let drop = h.len() - 50;
            h.drain(0..drop);
        }
    }
}

/// End-of-request sample from the observed instants:
/// prefill = [first_seen, prefill_end], decode = [prefill_end, close].
/// Each speed only covers its own phase (no dilution).
pub(crate) fn close_sample(p: &PrevSlot, close: std::time::Instant, cur_processed: u64, cur_decoded: u64) -> TimingSample {
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
        model: None,
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

pub(crate) fn has_speeds(s: &TimingSample) -> bool {
    s.prompt_tps.is_some() || s.predicted_tps.is_some()
}

/// Exact speeds of a task from the server log, written at the end of
/// each request (`slot print_timing ... task <id> ...`):
/// (prompt_n, prompt_ms, predicted_n, predicted_ms). The log is the only
/// exact and immediate source (/metrics counters flush late).
///
/// In router mode every model runs in its own instance process, and task
/// ids restart per instance — `task 5` exists once per loaded model. The
/// instance prefix `[<port>]` that llama.cpp puts on each log line is
/// therefore **required** to attribute a line to the right model
/// (`port` = the instance port of the model, see `parse_model_ports`).

pub(crate) fn finish_sample(
    p: &PrevSlot,
    close: std::time::Instant,
    cur_processed: u64,
    cur_decoded: u64,
    log_path: Option<&std::path::Path>,
    win_start: Option<Ctr>,
    ctr_now: Ctr,
    port: Option<u16>,
) -> (TimingSample, bool, bool) {
    let mut s = close_sample(p, close, cur_processed, cur_decoded);
    let mut pre_ok = false;
    let mut dec_ok = false;
    if let (Some(tid), Some(lp)) = (p.task, log_path) {
        if let Some((pn, pms, dn, dms)) = lookup_log_timings(lp, tid, port) {
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
pub(crate) fn with_counters(mut s: TimingSample, start: Option<Ctr>, end: Ctr) -> (TimingSample, bool, bool) {
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


