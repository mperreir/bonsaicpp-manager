//! One activity poll: the whole live-tracking algorithm applied to a
//! single model (its own slots, counters, window, averages, history rows).
//! ctivity.rs drives it for every registered model.

use super::endpoint::hist_row_mut;
use super::prom::{parse_metric, parse_metric_u64};
use super::rate::{hold_or_stale, prefill_over, window_rate, WIN_SECS};
use super::sample::{finish_sample, has_speeds, push_history_into, session_cache, with_counters};
use super::types::{
    Ctr, PollAvg, PrevSlot, SimpleTrack, SlotLive, TimingSample, Pending,
};

fn slot_u64(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// Live state of one model known to the router.

#[derive(Debug, Clone)]
pub(crate) struct ModelOut {
    pub(crate) prefill_tps: Option<f64>,
    pub(crate) decode_tps: Option<f64>,
    pub(crate) avg_prefill_tps: Option<f64>,
    pub(crate) avg_decode_tps: Option<f64>,
    pub(crate) avg_prefill_polls: u64,
    pub(crate) avg_decode_polls: u64,
    pub(crate) cache_live: Option<f64>,
    pub(crate) cache_session: Option<f64>,
    pub(crate) cache_session_cached: u64,
    pub(crate) cache_session_total: u64,
    pub(crate) spec_accept: Option<f64>,
    pub(crate) spec_accepted: u64,
    pub(crate) spec_drafted: u64,
    pub(crate) ttft_last: Option<f64>,
    pub(crate) ttft_avg: Option<f64>,
    pub(crate) ttft_n: u64,
    pub(crate) active_slots: Vec<SlotLive>,
    pub(crate) ctx_used: u64,
    pub(crate) ctx_max: Option<u64>,
    pub(crate) requests_processing: Option<u64>,
    pub(crate) requests_deferred: Option<u64>,
}

/// One model's poll: the whole live-tracking algorithm, unchanged, over
/// that model's own slots, counters, history and averages.
/// `key` is the router alias (empty string in single-model mode).
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_model(
    key: &str,
    track: &mut SimpleTrack,
    avg: &mut PollAvg,
    hist: &mut Vec<TimingSample>,
    mtext: &str,
    slots_json: Option<&serde_json::Value>,
    ctr_now: Ctr,
    log_path: Option<&std::path::Path>,
    port: Option<u16>,
    now: std::time::Instant,
) -> ModelOut {
    let model: Option<String> = if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    };
    // `now` is the shared poll instant: one timeline for every model.

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
                            // Late spec flush of the previous request: amend
                            // its own history row, not another model's.
                            if let Some(e) = hist_row_mut(hist, &pend.wall.at, &model) {
                                if e.spec_drafted.is_none() {
                                    e.spec_drafted = better.spec_drafted;
                                    e.spec_accepted = better.spec_accepted;
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
                                port,
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
                            port,
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
    // Live: sliding-window rates, summed over active slots.
    // `None` when idle (no slot in phase with a fresh value) -> `--`.
    let prefill_tps = has_pre.then_some(smooth_pre_sum);
    let decode_tps = has_dec.then_some(smooth_dec_sum);

    // Speculative acceptance, per model (counters of this instance).
    let spec_accepted =
        parse_metric(mtext, "llamacpp:spec_decode_num_accepted_tokens_total").unwrap_or(0.0).max(0.0)
            as u64;
    let spec_drafted =
        parse_metric(mtext, "llamacpp:spec_decode_num_draft_tokens_total").unwrap_or(0.0).max(0.0)
            as u64;
    let spec_accept = if spec_drafted > 0 {
        Some((spec_accepted as f64 / spec_drafted as f64).clamp(0.0, 1.0))
    } else {
        None
    };

    // Averages over reads, per model: each poll with a speed counts as 1.
    if let Some(v) = prefill_tps {
        avg.sum_pre += v;
        avg.n_pre += 1;
    }
    if let Some(v) = decode_tps {
        avg.sum_dec += v;
        avg.n_dec += 1;
    }
    let avg_pre = if avg.n_pre > 0 {
        Some(avg.sum_pre / avg.n_pre as f64)
    } else {
        None
    };
    let avg_dec = if avg.n_dec > 0 {
        Some(avg.sum_dec / avg.n_dec as f64)
    } else {
        None
    };

    for mut s in finished {
        s.model = model.clone();
        push_history_into(hist, &mut s);
    }
    // Catch-up: counters may be flushed just after our closing poll's
    // /metrics read. Replay the window on the following idle polls
    // (safe attribution: nothing else is running). Stays alive until
    // speeds, cache AND spec are all resolved (bounded by `left`).
    if let Some((st, wall)) = amend {
        let (better, pre_now, dec_now) = with_counters(wall.clone(), st, ctr_now);
        let cache_now = better.cache_n.is_some();
        let spec_now = better.spec_drafted.is_some();
        if has_speeds(&better) {
            if let Some(e) = hist_row_mut(hist, &wall.at, &model) {
                *e = better;
            }
        }
        if let Some(p) = track.pending.as_mut() {
            p.pre_ok = p.pre_ok || pre_now;
            p.dec_ok = p.dec_ok || dec_now;
            p.cache_ok = p.cache_ok || cache_now;
            p.spec_ok = p.spec_ok || spec_now;
            if p.left > 0 {
                p.left -= 1;
            }
            if p.left == 0 || p.resolved() {
                track.pending = None;
            }
        }
    }

    // Session figures over this model's own history rows only.
    let mine: Vec<TimingSample> = hist.iter().filter(|s| s.model == model).cloned().collect();
    let (sess_cached, sess_total) = session_cache(&mine);
    let cache_session = if sess_total > 0 {
        Some(sess_cached as f64 / sess_total as f64)
    } else {
        None
    };
    // Time to first token from finished requests (prompt phase time).
    let mut ttft_sum = 0.0f64;
    let mut ttft_n = 0u64;
    let mut ttft_last: Option<f64> = None;
    for s in &mine {
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

    ModelOut {
        prefill_tps,
        decode_tps,
        avg_prefill_tps: avg_pre,
        avg_decode_tps: avg_dec,
        avg_prefill_polls: avg.n_pre,
        avg_decode_polls: avg.n_dec,
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
        requests_processing: parse_metric_u64(mtext, "llamacpp:requests_processing"),
        requests_deferred: parse_metric_u64(mtext, "llamacpp:requests_deferred"),
    }
}


