//! Sliding-window rate maths (pure functions, no state).

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


