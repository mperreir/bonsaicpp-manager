//! Dynamic parsing of `llama-server --help`.
//!
//! No parameter is hardcoded: the installed binary is queried
//! on every launch and the UI is generated from the resulting schema.
//!
//! Real format (verified on the prism binary, 700+ lines):
//! ```text
//! ----- common params -----
//! -t,    --threads N                      number of CPU threads ... (default: -1)
//!                                          (env: LLAMA_ARG_THREADS)
//! -n,    --predict, --n-predict N          number of tokens to predict ...
//! --rope-scaling {none,linear,yarn}        RoPE frequency scaling method ...
//!                                          the model
//! ```
//! Rules: options in column 0 (short `-x` + long `--x` + values),
//! descriptions continued on indented lines, sections `----- xxx -----`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelpOption {
    /// Canonical long form, e.g. "--threads" (first `--long`).
    pub flag: String,
    /// Other forms: shorts (`-t`) + long aliases (`--n-predict`).
    pub aliases: Vec<String>,
    /// Raw value name(s), e.g. "N", "KEY=TYPE:VALUE,...", "START END".
    pub value_name: Option<String>,
    pub description: String,
    pub default: Option<String>,
    pub kind: OptionKind,
    pub section: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    Bool,
    Int,
    Float,
    Path,
    Url,
    Enum(Vec<String>),
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelpSchema {
    pub sections: Vec<String>,
    pub options: Vec<HelpOption>,
    pub raw_version: Option<String>,
    /// Raw text (for the `<details>` debug panel).
    pub raw: String,
    pub parsed_at: String,
}

fn split_variants(s: &str) -> Vec<String> {
    s.split([',', '|'])
        .map(|v| {
            v.trim()
                .trim_matches(|c| c == '{' || c == '}' || c == '[' || c == ']' || c == '<' || c == '>' || c == '\'' || c == '"')
                .to_string()
        })
        .filter(|v| !v.is_empty())
        .collect()
}

fn guess_kind(flag: &str, value: Option<&str>, default: Option<&str>) -> OptionKind {
    let v = match value {
        None => return OptionKind::Bool,
        Some(v) => v,
    };
    // explicit enums: {a,b}  [a|b]  <a|b>
    if (v.contains('{') && v.contains('}')) || (v.contains('[') && v.contains('|')) || (v.contains('<') && v.contains('|')) {
        let vals = split_variants(v);
        if vals.len() >= 2 {
            return OptionKind::Enum(vals);
        }
    }
    // Bare comma-separated literal lists, e.g. `--spec-type`
    // `none,draft-simple,...,ngram-cache` (no `{}`/`[]` markers).
    // Uppercase placeholders (`N0,N1,...`, `TOOL1,TOOL2,...`) and `<...>`
    // patterns (`<dev1,dev2,..>`) stay text: only all-lowercase literals
    // are enum options.
    if v.contains(',') && !v.chars().any(|c| matches!(c, '<' | '>' | '[' | ']' | '{' | '}')) {
        let vals: Vec<String> = split_variants(v)
            .into_iter()
            .filter(|s| !s.chars().all(|c| c == '.'))
            .collect();
        if vals.len() >= 2
            && vals.iter().all(|s| {
                !s.is_empty()
                    && s
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            })
        {
            return OptionKind::Enum(vals);
        }
    }
    let vu = v.to_uppercase();
    for tok in vu.split_whitespace() {
        if ["PATH", "FNAME", "FILE", "DIRECTORY", "DIR"].iter().any(|t| tok.contains(t)) {
            return OptionKind::Path;
        }
        if tok.contains("URL") {
            return OptionKind::Url;
        }
    }
    // the default decides int vs float when it is numeric
    if let Some(d) = default {
        let t = d.trim().trim_matches(['\'', '"']);
        if t.parse::<i64>().is_ok() {
            return OptionKind::Int;
        }
        if t.parse::<f64>().is_ok() {
            return OptionKind::Float;
        }
    }
    const FLOAT_FLAGS: [&str; 9] = [
        "temp", "top-p", "min-p", "penalty", "rope-freq-scale", "tfs", "typical", "mirostat-eta", "dynatemp",
    ];
    if FLOAT_FLAGS.iter().any(|f| flag.contains(f)) {
        return OptionKind::Float;
    }
    // `N` (llama convention = number) means int; any other all-caps
    // (`TYPE`, `STRING`, `NAME`, ...) is text: a numeric field breaks
    // input (e.g. --cache-type-k TYPE expects "f16", "q8_0"...). Text
    // also accepts digits, the reverse is false.
    if v == "N" {
        return OptionKind::Int;
    }
    OptionKind::Text
}

fn extract_default(desc: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\(default:\s*([^)]+)\)").unwrap();
    re.captures(desc).map(|c| c[1].trim().to_string())
}

/// Does a token look like a value name (`N`, `FNAME`, `lo-hi`,
/// `{a,b}`, `[a|b]`, `<a|b>`, `KEY=...`, `none,draft-simple,...`)
/// rather than prose?
fn is_value_token(t: &str) -> bool {
    if t.is_empty() {
        return false;
    }
    let c0 = t.chars().next().unwrap();
    if c0.is_ascii_uppercase() || c0 == '{' || c0 == '[' || c0 == '<' {
        return true;
    }
    // Bare comma-separated value lists, e.g. `--spec-type`
    // `none,draft-simple,...,ngram-cache` (its help line wraps, so the
    // continuation line starts with a leading `,`).
    if t.contains(',') {
        let u = t.trim_start_matches(',');
        if !u.is_empty()
            && u.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ',' | '.' | ':' | '/' | '+' | '|')
            })
        {
            return true;
        }
    }
    // `lo-hi` style
    if t.contains('-')
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return true;
    }
    false
}

/// Try to parse an option line (column 0, starts with `-`).
/// Returns (canonical, aliases, values, inline_description).
fn parse_option_line(line: &str) -> Option<(String, Vec<String>, Option<String>, String)> {
    if !line.starts_with('-') {
        return None;
    }
    // 1. Flags prefix: one or more `-x,` / `--long,` forms (the alignment
    //    padding between `-t,` and `--threads` contains spaces!).
    let flags_re = Regex::new(r"^(?P<flags>(?:-{1,2}[A-Za-z0-9?][A-Za-z0-9\-?]*\s*,?\s*)+)").unwrap();
    let caps = flags_re.captures(line)?;
    let flags_part = caps.name("flags")?.as_str();
    let rest = line[flags_part.len()..].trim_start().to_string();

    // 2. Classify the forms.
    let mut longs: Vec<String> = vec![];
    let mut shorts: Vec<String> = vec![];
    for tok in flags_part.split_whitespace() {
        let t = tok.trim_end_matches(',');
        if t.starts_with("--") && t.len() > 2 {
            longs.push(t.to_string());
        } else if t.starts_with('-') && t.len() > 1 && t.len() <= 5 && !t[1..].starts_with('-') {
            shorts.push(t.to_string());
        }
    }
    if longs.is_empty() {
        return None; // e.g. bullet "- auto: ..."
    }

    // 3. Values: leading tokens of the rest that look like placeholders.
    //    The description is always separated by 2+ spaces in this format.
    let gap = Regex::new(r"\s{2,}").unwrap();
    let (maybe_values, after_gap) = match gap.find(&rest) {
        Some(m) => (rest[..m.start()].to_string(), rest[m.end()..].trim().to_string()),
        None => (rest.clone(), String::new()),
    };
    let mut values: Vec<String> = vec![];
    let no_gap = after_gap.is_empty();
    let mut desc = after_gap;
    let vtoks: Vec<&str> = maybe_values.split_whitespace().collect();
    let mut i = 0;
    while i < vtoks.len() && is_value_token(vtoks[i]) {
        // With no separating gap, only accept if ALL tokens are
        // values (e.g. "START END" at end of line).
        if no_gap && vtoks[i + 1..].iter().any(|t| !is_value_token(t)) {
            break;
        }
        values.push(vtoks[i].to_string());
        i += 1;
    }
    if i < vtoks.len() {
        let mut d = vtoks[i..].join(" ");
        if !desc.is_empty() {
            d.push(' ');
            d.push_str(&desc);
        }
        desc = d;
    }
    let mut aliases = shorts;
    aliases.extend(longs.iter().skip(1).cloned());
    let value_name = if values.is_empty() {
        None
    } else {
        Some(values.join(" "))
    };
    Some((longs[0].clone(), aliases, value_name, desc))
}

/// Parse the raw `--help` output.
pub fn parse_help(text: &str) -> HelpSchema {
    // Possible BOM (file-redirection artifact, never via pipe).
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let section_re = Regex::new(r"^\s*-{3,}\s*(.+?)\s*-{3,}\s*$").unwrap();
    let colon_section_re = Regex::new(r"^([A-Za-z][A-Za-z0-9 /_\-]{1,50}):$").unwrap();

    let mut sections: Vec<String> = vec!["General".to_string()];
    let mut options: Vec<HelpOption> = vec![];
    let mut current = "General".to_string();
    let mut pending: Option<(String, Vec<String>, Option<String>, String)> = None;
    let mut pending_desc = String::new();

    let flush = |pending: &mut Option<(String, Vec<String>, Option<String>, String)>,
                     desc_extra: &mut String,
                     options: &mut Vec<HelpOption>,
                     current: &str| {
        if let Some((flag, aliases, vn, first)) = pending.take() {
            let full = if desc_extra.is_empty() {
                first
            } else if first.is_empty() {
                desc_extra.trim().to_string()
            } else {
                format!("{first} {}", desc_extra.trim())
            };
            let default = extract_default(&full);
            let kind = guess_kind(&flag, vn.as_deref(), default.as_deref());
            options.push(HelpOption {
                flag,
                aliases,
                value_name: vn,
                description: full,
                default,
                kind,
                section: current.to_string(),
            });
            desc_extra.clear();
        }
    };

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        // Section: only if the line starts in column 0 (never an
        // indented continuation, never a "- ..." bullet).
        if !line.starts_with(' ') && !line.starts_with('\t') {
            if let Some(c) = section_re.captures(line) {
                flush(&mut pending, &mut pending_desc, &mut options, &current);
                current = c[1].trim().to_string();
                if !sections.contains(&current) {
                    sections.push(current.clone());
                }
                continue;
            }
            if let Some(c) = colon_section_re.captures(line.trim()) {
                flush(&mut pending, &mut pending_desc, &mut options, &current);
                current = c[1].trim().to_string();
                if !sections.contains(&current) {
                    sections.push(current.clone());
                }
                continue;
            }
            if let Some((flag, aliases, vn, desc)) = parse_option_line(line) {
                flush(&mut pending, &mut pending_desc, &mut options, &current);
                pending = Some((flag, aliases, vn, desc));
                continue;
            }
            // Unrecognized column-0 line (e.g. "usage: ..."): closes the
            // current option but is neither a section nor a continuation.
            // Exception: a wrapped value list (the help printer splits a very
            // long comma-separated value onto the next line, e.g. `--spec-type`
            // `...ngram-map-k4v` followed by `,ngram-mod,ngram-cache`).
            if let Some((_, _, vn, first)) = pending.as_mut() {
                let toks: Vec<&str> = line.split_whitespace().collect();
                if first.is_empty()
                    && pending_desc.is_empty()
                    && !toks.is_empty()
                    && toks.iter().all(|t| is_value_token(t))
                {
                    let extra = toks.join(" ");
                    match vn {
                        Some(v) => {
                            if v.ends_with(',') || extra.starts_with(',') {
                                v.push_str(&extra);
                            } else {
                                v.push(' ');
                                v.push_str(&extra);
                            }
                        }
                        None => *vn = Some(extra),
                    }
                    continue;
                }
            }
            flush(&mut pending, &mut pending_desc, &mut options, &current);
        } else if pending.is_some() {
            if !pending_desc.is_empty() {
                pending_desc.push(' ');
            }
            pending_desc.push_str(line.trim());
        }
    }
    flush(&mut pending, &mut pending_desc, &mut options, &current);
    // Drop "General" if it stayed empty (real header present from the start).
    if !options.iter().any(|o| o.section == "General") {
        sections.retain(|s| s != "General");
    }

    HelpSchema {
        sections,
        options,
        raw_version: text.lines().find(|l| !l.trim().is_empty()).map(|s| s.to_string()),
        raw: text.to_string(),
        parsed_at: chrono::Local::now().to_rfc3339(),
    }
}

/// Tauri command: run `<exe> --help` and return the parsed schema.
/// Cache of the last parsed `--help`, keyed by the binary it came from.
/// Running `llama-server --help` costs a process spawn (hundreds of ms on
/// a cold binary), and the router editor validates on every keystroke it
/// commits — without this, typing an alias froze the field. A backend
/// update changes the path, which misses the cache automatically.
static SCHEMA_CACHE: OnceLock<Mutex<Option<(std::path::PathBuf, HelpSchema)>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<(std::path::PathBuf, HelpSchema)>> {
    SCHEMA_CACHE.get_or_init(|| Mutex::new(None))
}

/// Drop the cached schema (after a backend install/switch).
pub fn invalidate_schema_cache() {
    if let Ok(mut g) = cache().lock() {
        *g = None;
    }
}

/// Run `llama-server --help` on the installed backend and parse it, reusing
/// the cached result while the binary is unchanged.
pub async fn fetch_schema(app: &tauri::AppHandle) -> Result<HelpSchema, String> {
    use crate::backend::find_installed;
    use crate::locale::{lang_of, msg, Msg};
    use crate::state::backend_root;
    let root = backend_root(app)?;
    let (_, _, exe) = find_installed(&root).ok_or(msg(lang_of(app), Msg::BackendNotInstalled))?;

    if let Ok(g) = cache().lock() {
        if let Some((path, schema)) = g.as_ref() {
            if *path == exe {
                return Ok(schema.clone());
            }
        }
    }

    // No console flash in release (tokio::process::Command has creation_flags()).
    let mut help_cmd = tokio::process::Command::new(&exe);
    #[cfg(windows)]
    help_cmd.creation_flags(0x08000000);
    let out = help_cmd
        .arg("--help")
        .output()
        .await
        .map_err(|e| format!("execution --help: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&out.stderr).to_string();
    }
    if text.trim().is_empty() {
        return Err("le binaire n'a renvoye aucune aide".to_string());
    }
    let schema = parse_help(&text);
    if let Ok(mut g) = cache().lock() {
        *g = Some((exe, schema.clone()));
    }
    Ok(schema)
}

#[tauri::command]
pub async fn parse_server_help(app: tauri::AppHandle) -> Result<HelpSchema, String> {
    fetch_schema(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Excerpt copied from the real `--help` (prism binary). Careful: no `\`
    /// at end of line here, as it would eat the significant indentation;
    /// the 8 spaces of source formatting are stripped by the test.
    const SAMPLE: &str = "----- common params -----
        -h,    --help, --usage                  print usage and exit
        -t,    --threads N                      number of CPU threads to use during generation (default: -1)
                                                         (env: LLAMA_ARG_THREADS)
        -n,    --predict, --n-predict N          number of tokens to predict (default: -1, -1 = infinity)
        --cpu-strict <0|1>                      use strict CPU placement (default: 0)
        --rope-scaling {none,linear,yarn}        RoPE frequency scaling method, defaults to linear unless specified by
                                                         the model
                                                         (env: LLAMA_ARG_ROPE_SCALING_TYPE)
        -m,    --model FNAME                    model path
        --control-vector-layer-range START END
                                                         layer range to apply the control vector(s)
        -ctk,  --cache-type-k TYPE              KV cache data type for K
        ----- sampling params -----
        --temp N                                temperature (default: 0.8)
";

    #[test]
    fn parses_real_format() {
        // The sample lines are indented by 8 spaces due to source
        // formatting: strip that prefix to recover the real format
        // (options in column 0, indented continuations).
        let sample: String = SAMPLE
            .lines()
            .map(|l| l.strip_prefix("        ").unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        let s = parse_help(&sample);
        assert_eq!(s.sections, vec!["common params", "sampling params"]);
        assert!(s.options.len() >= 8, "options: {}", s.options.len());

        let threads = s.options.iter().find(|o| o.flag == "--threads").unwrap();
        assert_eq!(threads.aliases, vec!["-t".to_string()]);
        assert_eq!(threads.value_name.as_deref(), Some("N"));
        assert_eq!(threads.default.as_deref(), Some("-1"));
        assert_eq!(threads.kind, OptionKind::Int);
        assert!(threads.description.contains("LLAMA_ARG_THREADS"));

        let help = s.options.iter().find(|o| o.flag == "--help").unwrap();
        assert!(help.aliases.contains(&"-h".to_string()));
        assert!(help.aliases.contains(&"--usage".to_string()));
        assert_eq!(help.kind, OptionKind::Bool);

        let rope = s.options.iter().find(|o| o.flag == "--rope-scaling").unwrap();
        assert_eq!(
            rope.kind,
            OptionKind::Enum(vec!["none".into(), "linear".into(), "yarn".into()])
        );
        assert!(rope.description.contains("the model"), "desc: {}", rope.description);

        let strict = s.options.iter().find(|o| o.flag == "--cpu-strict").unwrap();
        assert_eq!(strict.kind, OptionKind::Enum(vec!["0".into(), "1".into()]));

        let model = s.options.iter().find(|o| o.flag == "--model").unwrap();
        assert_eq!(model.kind, OptionKind::Path);

        let cv = s.options.iter().find(|o| o.flag == "--control-vector-layer-range").unwrap();
        assert_eq!(cv.value_name.as_deref(), Some("START END"));
        assert!(cv.description.contains("layer range"));

        let temp = s.options.iter().find(|o| o.flag == "--temp").unwrap();
        assert_eq!(temp.kind, OptionKind::Float);
        assert_eq!(temp.section, "sampling params");

        // `TYPE` = string (f16, q8_0...), not a number.
        let ctk = s.options.iter().find(|o| o.flag == "--cache-type-k").unwrap();
        assert_eq!(ctk.value_name.as_deref(), Some("TYPE"));
        assert_eq!(ctk.kind, OptionKind::Text);
    }

    #[test]
    fn spec_type_is_enum_not_bool() {
        // Exact lines from the real binary (`llama-server --help`): the value
        // list is so long the printer wraps it onto a column-0 continuation
        // line starting with `,` (built with join to keep column-0 exact).
        let lines = vec![
            "--spec-type none,draft-simple,draft-eagle3,draft-mtp,draft-dflash,draft-dspark,ngram-simple,ngram-map-k,ngram-map-k4v",
            ",ngram-mod,ngram-cache",
            "                                          comma-separated list of types of speculative decoding to use (default:",
            "                                          none)",
            "                                          (env: LLAMA_ARG_SPEC_TYPE)",
        ];
        let s = parse_help(&lines.join("\n"));
        assert_eq!(s.options.len(), 1);
        let o = &s.options[0];
        assert_eq!(o.flag, "--spec-type");
        assert_eq!(
            o.kind,
            OptionKind::Enum(vec![
                "none".into(),
                "draft-simple".into(),
                "draft-eagle3".into(),
                "draft-mtp".into(),
                "draft-dflash".into(),
                "draft-dspark".into(),
                "ngram-simple".into(),
                "ngram-map-k".into(),
                "ngram-map-k4v".into(),
                "ngram-mod".into(),
                "ngram-cache".into(),
            ])
        );
        assert!(o.description.contains("comma-separated"), "desc: {}", o.description);
        assert_eq!(o.default.as_deref(), Some("none"));
    }

    #[test]
    fn uppercase_comma_placeholders_stay_text() {
        let s = parse_help("--tensor-split N0,N1,N2,...  fraction of the model to offload\n");
        assert_eq!(s.options.len(), 1);
        assert_eq!(s.options[0].kind, OptionKind::Text);
        let s = parse_help("--device <dev1,dev2,..>  comma-separated list of devices\n");
        assert_eq!(s.options.len(), 1);
        assert_eq!(s.options[0].kind, OptionKind::Text);
    }

    #[test]
    fn ignores_env_only_continuation_as_option() {
        // A lone "(env: ...)" or "(default: ...)" must not create an option.
        let s = parse_help("----- common params -----\n--prio N   prio (default: 0)\n   (env: LLAMA_ARG_PRIO)\n");
        assert_eq!(s.options.len(), 1);
    }
}
