//! Shared data model: request samples, per-model tracking state,
//! and the activity payloads returned to the frontend.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

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
    /// Router alias of the model that served the request. `None` in
    /// single-model mode (the legacy key).
    #[serde(default)]
    pub model: Option<String>,
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
    /// Averages over polls, per model (memory only).
    pub avg: Mutex<HashMap<String, PollAvg>>,
    /// Per-request history (memory only, last 50), each row tagged with
    /// the model that served it.
    pub hist: Mutex<Vec<TimingSample>>,
    /// Slot tracking between two polls, per model: the router can have
    /// several models loaded at once, each with its own slots, its own
    /// counters and its own request lifecycle.
    pub track: Mutex<HashMap<String, SimpleTrack>>,
}

/// Track key of the single-model (non-router) mode.
pub const SINGLE_MODEL_KEY: &str = "";

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
    pub(crate) start: Option<Ctr>,
    pub(crate) wall: TimingSample,
    pub(crate) left: u8,
    pub(crate) pre_ok: bool,
    pub(crate) dec_ok: bool,
    pub(crate) cache_ok: bool,
    pub(crate) spec_ok: bool,
}

impl Pending {
    /// Everything counter-resolvable arrived: no more replay needed.
    pub(crate) fn resolved(&self) -> bool {
        self.pre_ok && self.dec_ok && self.cache_ok && self.spec_ok
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Ctr {
    pub(crate) pt: f64,
    pub(crate) ps: f64,
    pub(crate) dt: f64,
    pub(crate) ds: f64,
    pub(crate) ct: f64,
    pub(crate) sd: f64,
    pub(crate) sa: f64,
}


#[derive(Clone)]
pub(crate) struct PrevSlot {
    pub(crate) task: Option<u64>,
    pub(crate) processing: bool,
    pub(crate) prompt_total: u64,
    pub(crate) processed: u64,
    pub(crate) decoded: u64,
    pub(crate) first_seen: std::time::Instant,
    /// Prefill -> decode switch instant (None if never observed).
    pub(crate) prefill_end: Option<std::time::Instant>,
    /// /metrics counters when the request appeared (exact window).
    pub(crate) start: Option<Ctr>,
    /// True if another request was running in parallel (shared window).
    pub(crate) shared: bool,
    /// Recent (poll time, processed, decoded) snapshots for the
    /// sliding-window live rate (pruned to `WIN_SECS`).
    pub(crate) snaps: Vec<(std::time::Instant, u64, u64)>,
    /// Last windowed rate per phase, held while in phase so the display
    /// doesn't flicker `--`. `None` when idle or stale.
    pub(crate) smooth_pre: Option<f64>,
    pub(crate) smooth_dec: Option<f64>,
    /// Last poll with observed progress, per phase (staleness timeout).
    pub(crate) last_pre_at: Option<std::time::Instant>,
    pub(crate) last_dec_at: Option<std::time::Instant>,
    /// Cache counts frozen at the last prefill poll (same instant, same
    /// source — the only coherent pair): cached prefix + prompt size.
    /// Decoding resets the slot counters, so these (not live values)
    /// feed the frozen display and the request history.
    pub(crate) cache_final_n: Option<u64>,
    pub(crate) cache_final_total: Option<u64>,
}

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
pub struct ModelActivity {
    /// Router alias (INI section name) used by API clients.
    pub alias: String,
    /// `unloaded` | `loading` | `loaded` | `sleeping` (empty in
    /// single-model mode).
    pub status: String,
    /// At least one slot is decoding.
    pub active: bool,
    pub prefill_tps: Option<f64>,
    pub decode_tps: Option<f64>,
    pub ctx_used: u64,
    pub ctx_max: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Activity {
    /// The server runs in router mode (several models, loaded on demand).
    pub router: bool,
    /// Per-model live state, one entry per registered model.
    pub models: Vec<ModelActivity>,
    /// Alias the flat fields below describe (`None` in single-model mode).
    pub selected_model: Option<String>,
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
    /// Running slots + what they are doing (of the selected model).
    pub active_slots: Vec<SlotLive>,
    /// Context: tokens used (active slots) / n_ctx (of the selected model).
    pub ctx_used: u64,
    pub ctx_max: Option<u64>,
    pub requests_processing: Option<u64>,
    pub requests_deferred: Option<u64>,
    /// Per-request history (backend, memory only), rows tagged with
    /// their model.
    pub history: Vec<TimingSample>,
}

impl Activity {
    /// Nothing to report yet (router with no registered model, or the
    /// server answers nothing yet).
    pub(crate) fn empty(router: bool) -> Self {
        Self {
            router,
            models: vec![],
            selected_model: None,
            prefill_tps: None,
            decode_tps: None,
            avg_prefill_tps: None,
            avg_decode_tps: None,
            avg_prefill_polls: 0,
            avg_decode_polls: 0,
            cache_live: None,
            cache_session: None,
            cache_session_cached: 0,
            cache_session_total: 0,
            spec_accept: None,
            spec_accepted: 0,
            spec_drafted: 0,
            ttft_last: None,
            ttft_avg: None,
            ttft_n: 0,
            active_slots: vec![],
            ctx_used: 0,
            ctx_max: None,
            requests_processing: None,
            requests_deferred: None,
            history: vec![],
        }
    }
}


