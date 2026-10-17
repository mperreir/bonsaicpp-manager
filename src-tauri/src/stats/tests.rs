//! Unit tests for the whole stats module tree.
//! Kept in one file: the cases cross modules (a poll asserts on the
//! sample it produced and on the history it wrote), so splitting them
//! per file would scatter fixtures that belong together.

use super::*;
use super::endpoint::{hist_row_mut, parse_models_list, urlencode};
use super::log::{lookup_log_timings, parse_model_ports, parse_ms_n};
use super::prom::parse_metric;
use super::rate::{hold_or_stale, prefill_over, window_rate, STALE_SECS};
use super::sample::{cache_pair, close_sample, push_history, session_cache, with_counters};
use super::types::{Ctr, Pending, PrevSlot, TimingSample};


fn sample(prompt_n: u64, prompt_ms: f64, predicted_n: u64, predicted_ms: f64) -> TimingSample {
    TimingSample {
        at: "2026-01-01T00:00:00+00:00".to_string(),
        model: None,
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
fn router_model_ports_come_from_the_proxy_lines() {
    // Real router log lines.
    let log = "0.09.835 I srv  proxy_reques: proxying request to model test-9b on port 49574\n\
               1.30.571 I srv  proxy_reques: proxying request to model test-27b on port 57182\n\
               1.31.009 I srv  proxy_reques: proxying request to model test-9b on port 49600\n";
    let m = parse_model_ports(log);
    // Last occurrence wins: an instance restart changes the port.
    assert_eq!(m.get("test-9b").copied(), Some(49600));
    assert_eq!(m.get("test-27b").copied(), Some(57182));
}

#[test]
fn router_model_ports_ignore_other_lines() {
    let log = "[49574] I slot print_timing: id  3 | task 5 | n_gen = 100\n\
               I srv  proxy_reques: proxying request to model  on port \n";
    assert!(parse_model_ports(log).is_empty());
}

#[test]
fn log_timings_are_attributed_to_the_right_instance() {
    // Two instances both used task 5: without the `[port]` prefix the
    // first match would win and one model would steal the other's
    // timings. Layout taken from a real router run.
    let dir = std::env::temp_dir().join("bonsai-log-attr-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("llama-server.log");
    let log = "[49574] I slot print_timing: id  3 | task 5 | prompt eval time =    100.00 ms /    10 tokens (   10.00 ms per token)\n\
               [49574] I slot print_timing: id  3 | task 5 |        eval time =    2000.00 ms /   100 tokens (   20.00 ms per token)\n\
               [57182] I slot print_timing: id  0 | task 5 | prompt eval time =     10.00 ms /     1 tokens\n\
               [57182] I slot print_timing: id  0 | task 5 |        eval time =      50.00 ms /     5 tokens\n";
    std::fs::write(&path, log).unwrap();

    assert_eq!(
        lookup_log_timings(&path, 5, Some(49574)),
        Some((10, 100.0, 100, 2000.0))
    );
    assert_eq!(
        lookup_log_timings(&path, 5, Some(57182)),
        Some((1, 10.0, 5, 50.0))
    );
    // Unknown instance: nothing (never borrow another model's timings).
    assert_eq!(lookup_log_timings(&path, 5, Some(1234)), None);
    // Single-model mode (port = None): no prefix filter, last match
    // wins — unchanged legacy behaviour.
    assert_eq!(
        lookup_log_timings(&path, 5, None),
        Some((1, 10.0, 5, 50.0))
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn models_list_parses_alias_and_status() {
    // Real `GET /models` shape in router mode.
    let v = serde_json::json!({
        "data": [
            {"id": "test-9b", "status": {"value": "loaded", "args": ["--port","49574"]}},
            {"id": "test-27b", "status": {"value": "sleeping"}},
            {"id": "test-new", "status": {"value": "unloaded"}}
        ]
    });
    assert_eq!(
        parse_models_list(&v),
        vec![
            ("test-9b".to_string(), "loaded".to_string()),
            ("test-27b".to_string(), "sleeping".to_string()),
            ("test-new".to_string(), "unloaded".to_string()),
        ]
    );
}

#[test]
fn models_list_tolerates_missing_fields() {
    assert!(parse_models_list(&serde_json::json!({})).is_empty());
    assert!(parse_models_list(&serde_json::json!({"data": []})).is_empty());
    // No `status`: treated as not loaded rather than dropped.
    let v = serde_json::json!({"data": [{"id": "x"}]});
    assert_eq!(
        parse_models_list(&v),
        vec![("x".to_string(), "unloaded".to_string())]
    );
}

#[test]
fn alias_is_percent_encoded_for_the_query() {
    assert_eq!(urlencode("bonsai-27b"), "bonsai-27b");
    assert_eq!(urlencode("a/b:c"), "a%2Fb%3Ac");
    assert_eq!(urlencode("q8_0 model"), "q8_0%20model");
}

#[test]
fn history_rows_are_matched_on_model_too() {
    // The catch-up must amend the row of *its* model even when
    // another model's request finished in between.
    let mut a = sample(10, 100.0, 10, 100.0);
    a.at = "T1".into();
    a.model = Some("a".into());
    let mut b = sample(20, 200.0, 20, 200.0);
    b.at = "T2".into();
    b.model = Some("b".into());
    let mut c = sample(30, 300.0, 30, 300.0);
    c.at = "T3".into();
    c.model = Some("a".into());
    let mut h = vec![a, b, c];

    let row = hist_row_mut(&mut h, "T1", &Some("a".into())).unwrap();
    row.predicted_n = Some(999);
    assert_eq!(h[0].predicted_n, Some(999));
    assert_eq!(h[1].predicted_n, Some(20)); // untouched

    // Same `at`, other model: no match.
    assert!(hist_row_mut(&mut h, "T1", &Some("b".into())).is_none());
    assert!(hist_row_mut(&mut h, "T1", &None).is_none());
}

    // The user's case: req1 fresh (338 prompt, 0 cached), req2 with
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
