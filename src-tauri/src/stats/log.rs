//! Server log parsing: exact per-request timings, attributed to the
//! right model instance through the [<port>] line prefix.

use std::collections::HashMap;

pub(crate) fn lookup_log_timings(
    log_path: &std::path::Path,
    task: u64,
    port: Option<u16>,
) -> Option<(u64, f64, u64, f64)> {
    let bytes = std::fs::read(log_path).ok()?;
    let tail = if bytes.len() > 262144 {
        &bytes[bytes.len() - 262144..]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(tail);
    let key = format!("task {task} |");
    let prefix = port.map(|p| format!("[{p}]"));
    let mut prompt: Option<(u64, f64)> = None;
    let mut gen: Option<(u64, f64)> = None;
    for line in text.lines() {
        if !line.contains("print_timing") || !line.contains(&key) {
            continue;
        }
        // Instance attribution: the line must come from this model's
        // instance process.
        if let Some(pfx) = &prefix {
            if !line.starts_with(pfx.as_str()) {
                continue;
            }
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

/// alias -> instance port, from the router's own log lines:
/// `srv  proxy_reques: proxying request to model <alias> on port <p>`.
/// This is what lets `lookup_log_timings` attribute a `print_timing` line
/// to the right model (task ids restart in every instance).
/// Pure parser, tested.
pub(crate) fn parse_model_ports(log_text: &str) -> HashMap<String, u16> {
    let mut out: HashMap<String, u16> = HashMap::new();
    for line in log_text.lines() {
        if let Some(rest) = line.split_once("proxying request to model ") {
            if let Some((alias, tail)) = rest.1.split_once(" on port ") {
                if let Ok(p) = tail.trim().parse::<u16>() {
                    out.insert(alias.trim().to_string(), p);
                }
            }
        }
    }
    out
}

/// `139.31 ms / 4 tokens` -> (ms, n).
pub(crate) fn parse_ms_n(right: &str) -> Option<(f64, u64)> {
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
