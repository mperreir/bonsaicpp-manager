# AGENTS.md — Bonsai.cpp Manager

Guide for AI agents (and humans) working on this repo.
Windows app **Tauri 2 + React + TypeScript (Vite) + Rust**: installs and drives
the `llama-server` backend from the [PrismML-Eng/llama.cpp](https://github.com/PrismML-Eng/llama.cpp)
fork (Bonsai 2 models). See `README.md` for usage.

## Environment (Windows machine, PowerShell 5.1)

- **Node 24 via nvm-sh** (`~/.nvm`, not nvm-windows): `nvm`/`node`/`npx` are
  **not** on the default PATH. Prefix every command:
  `$env:PATH="C:\Users\nous\.nvm\versions\node\v24.18.0\bin;$env:PATH"`
- **Rust**: `$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"` (`cargo 1.99`, `stable` toolchain).
- **C++ build**: VS 2022 Build Tools + VCTools workload (required to link in debug/release).
- PowerShell 5.1: no `head` (use `Select-Object -First N`), no `<<` heredocs,
  `foreach` (not `for`), `select-string` ~ grep. `a.exe > f.txt` writes **UTF-16LE**.

## Commands

```powershell
npm install
npm run tauri dev      # vite :1420 + auto rebuild of the binary; closing the window = hide to tray
npm run build          # tsc + prod vite
cd src-tauri; cargo test --lib   # unit tests (--help parser, router INI, stats)
cd src-tauri; cargo test --test router_preset -- --ignored   # real llama-server on the generated INI
cd src-tauri; cargo check
```

Clean dev restart (the watcher doesn't always pick up `tauri.conf.json`):
kill `tauri.js dev`, `vite.js` and `bonsaicpp-manager.exe`, then relaunch.
A ghost tray icon after kill is normal (Explorer cache, disappears on hover).

## Architecture

- `src-tauri/src/`: `lib.rs` (tray + menu + single-instance + autostart + close→hide),
  `backend.rs` (prebuilt GitHub Releases `bin/<tag>/<flavor>/`, GPU detection:
  `nvidia-smi` then WMI names — AMD/Radeon → hip, Arc → vulkan; default persisted
  in `bin/default.txt` and honored by `find_installed`; install stops a running
  server first (shared `stop_server_impl`) and becomes default; delete refused
  while running; `is_server_running` covers tracked child + adopted orphan),
  `help_parser.rs` (`--help` parser + tests; `fetch_schema` is the reusable
  entry point), `models.rs` (Hugging Face downloads),
  `router.rs` (router registry + generation/validation of the
  `--models-preset` INI — see § Router mode),
  `server_cmd.rs` (`llama-server` supervision, forced `--metrics`; `server_status`
  **adopts** an orphan server answering on the last known port; `stop_server`
  also kills the `llama-server.exe` orphan via taskkill after checking its name),
  `stats.rs` is a **directory** `stats/` — see § Stats (module tree) —,
  `state.rs` (paths + server state + last port).
- `src/`: `App.tsx` (sidebar shell, `localStorage` presets), `lib/api.ts` (`invoke`
  wrappers + types), `hooks/useStats.ts` (App-level polling: survives tab switches
  and tray hiding — NEVER poll from a page),
  `pages/` (Backend, Models, Server Simple/Expert, Stats, Logs).
- Data: `%APPDATA%\com.bonsai.manager\` (**bundle identifier**, not product name):
  `bin/`, `models/bonsai2-gguf/`, `logs/llama-server.log`.

## Stats (rewritten 2026-10-10, 6-point user spec)

**Module tree** (`src-tauri/src/stats/`, split from a 2000-line `stats.rs`
on 2026-10-17). One concern per file, tests in `tests.rs`:

| file | concern |
|---|---|
| `types.rs` | `TimingSample`, `StatsState`, `PollAvg`, `SimpleTrack`, `PrevSlot`, `Pending`, `Ctr`, `Activity`, `ModelActivity`, `ModelOut` |
| `prom.rs` | `/metrics` Prometheus parsing (`parse_metric`, `read_ctrs`) + the `scrape_metrics` command |
| `rate.rs` | sliding-window maths, pure (`window_rate`, `hold_or_stale`, `prefill_over`, `WIN_SECS`) |
| `log.rs` | server log parsing, attributed per instance (`lookup_log_timings`, `parse_model_ports`) |
| `sample.rs` | one finished-request sample (`close_sample`, `with_counters`, `finish_sample`, history) |
| `endpoint.rs` | router HTTP layer (`is_router`, `fetch_model`, `urlencode`, `parse_models_list`) |
| `poll.rs` | `poll_model`: the live-tracking algorithm, for **one** model |
| `activity.rs` | `poll_activity`: the command, drives `poll` per model |
| `cmds.rs` | `record_timing`, `send_test_prompt`, `set_tray_tooltip` |
| `tests.rs` | all 60 unit tests (they cross modules — a poll asserts on the sample *and* the history it wrote) |

- Unit tests stay in-module (`#[cfg(test)] mod tests`) or in `tests.rs`:
  they exercise **private** helpers, which an integration test in `tests/`
  cannot reach. `tests/router_preset.rs` is the one real integration test
  (it launches the installed `llama-server` on the generated INI, `#[ignore]`).
- `pub mod` only on `activity`, `cmds`, `prom` (they hold
  `#[tauri::command]`s that `generate_handler!` resolves by path) and on
  `router` (so the integration test can call `render_ini`).

- **Live**: per-slot `/slots` snapshots over a 10 s sliding window
  (`window_rate`, needs ≥ 1 s span), summed over active slots
  (`prefill_tps`, `decode_tps`, `None` → `--` when idle). The window
  covers coarse batch jumps (`/slots` counters advance in bursts with
  multi-second gaps). Last rate held while in phase (`hold_or_stale`);
  no progress for > 15 s → `None`. Fresh tasks reset snapshots + rates.
  Prefill end is latched per task (`prefill_over`: **decoded-token delta**,
  never `processed == prompt_total` — this fork grows `n_prompt_tokens`
  in stages mid-prefill): the slot flips to decode so prefill live goes
  `--` and its average freezes instead of decaying during decode.
  Snapshots reset at the switch so early decode isn't diluted by
  prefill zeros; a staged boundary (`== total`, no generation yet)
  holds the prefill display instead of flickering. Only realtime source (counters flush at
  phase/request end with a **variable multi-second delay** — never expect a
  synchronous value from them).
 - **Per-poll averages**: each poll with a speed feeds backend-side `PollAvg`
   (sum/count) — never reset, still shown when idle.
- **Cache / spec / TTFT (0.6)**: `cache_live` = prompt-weighted mean over
  prefill-phase slots, **frozen** while decoding (`track.cache_held`,
  slot counters reset; cleared idle; fresh tasks never inherit),
  per-request counts frozen in `PrevSlot.cache_final_n/_total` (feeds
  history via `finish_sample` before the counter window). `cache_session`
  sums coherent pairs (`session_cache`, unknowns skipped): the fork
  counts evaluated-only in `prompt_tokens_total`, so the pair is
  (cached, evaluated + cached) — never ratio × counter-`prompt_n`.
  Per-request spec (`spec_drafted`/`spec_accepted`: `timings` for test
  prompts, counter-window deltas + catch-up otherwise), session spec
  from `spec_decode_*` counters, history Spec column, dedupe keyed on
  speeds only. `ttft_last`/`ttft_avg`/`ttft_n` from history `prompt_ms`.
  StatsPage: two labeled sections (live / averages-session, 5 cards
  each, auto-fit grid), history adds Cache + TTFT + Spec columns.
- **`/metrics` parsing**: `parse_metric` must `continue` (never `?`) on
  mismatch — an early `?` aborted the scan so only a first-placed
  metric ever parsed (all counters silently 0; caught by
  `spec_counters_parse_beside_per_pos_lines`).
- **Slots**: `active_slots[]` (`prefill` while prompt advances and no
  generation observed, else `decode` — see `prefill_over` latch)
  + per-slot speed; tray tooltip = active speeds (backend sums), checkbox.
- **Context**: `ctx_used` = Σ over slots (`n_prompt_tokens` + `n_decoded`, active
  **and** idle since the cache survives); `ctx_max` = `n_ctx` (**shared KV cache**,
  not ×slot count).
- **History**: stored **backend-side** (`Vec`, last 50, memory only, returned on
  every poll) — the frontend renders `activity.history`, **no React-side
  accumulation/dedupe** (an impure `setHistory` updater + StrictMode emptied it).
- **Per-request speeds** (`finish_sample`): 1. **server log** (`print_timing …
  task <id>` → `lookup_log_timings`, exact and immediate), 2. `/metrics` counters
  over the [idle-baseline → end] window (`with_counters`, cache-excluded tokens),
  3. phase wall time (`close_sample` with a prefill→decode switch instant).
  `pending` catch-up (30 idle polls) when flushes land after the close;
  stays alive until speeds, cache AND spec are all resolved
  (`Pending::resolved`); a new task (`fresh`) replays the old window
  once for spec only (safe attribution) before handing over. Parallel requests: `shared` flag, observed window.
- **History duplicates**: `push_history_into` normalizes ms to 2 decimals and drops a
  sample identical to the last 3 (test request seen via `timings` + slot).

## Router mode (v0.8 — the default)

`llama-server` is launched **without `--model`**: nothing is loaded at
startup, each model is loaded by the first API call that names it in the
OpenAI `model` field (or `?model=` on GET endpoints).

- **Where the parameters come from**: a single INI passed at startup via
  `--models-preset` (`<appdata>/config/models.ini`, written by
  `router.rs::write_preset`). Two levels, same vocabulary as `--help`:
  `[*]` = defaults shared by every model, `[alias]` = one model + its
  overrides. llama.cpp precedence: **CLI args > `[alias]` > `[*]`**.
- **Alias = INI section name** = the name clients send. Generated by
  `suggestAlias()` (GGUF filename minus size/quant noise), deduplicated.
- **`routed` flag, not a list**: every downloaded GGUF has a `RouterModel`
  entry; `routed` decides whether it is published. `render_ini`,
  `validate` and `is_router` all go through `RouterConfig::routed()`, so an
  un-routed model is invisible to llama.cpp **and** its overrides are not
  validated (they never reach the preset). Un-ticking keeps the alias and
  the overrides: re-exposing restores them. `sync_with(paths, alias_of)`
  reconciles the registry with the disk (adds new files un-routed, drops
  vanished ones) — a download never publishes a model silently.
- **An unknown key is fatal**: llama.cpp exits 1 with
  `failed to initialize router models: option 'X' not recognized in preset 'Y'`.
  `router::validate` therefore rejects every key not in the `--help` schema
  *before* writing (`set_router_config` saves nothing on error), and
  `start_server` re-validates + reads the log back after 1.2 s to surface
  `Msg::PresetRejected` with llama.cpp's own wording.
- **Never written to the preset**: `host`, `port`, `api-key`, `alias`,
  `hf-repo`, `models-*`, `sleep-idle-seconds` (router-controlled — the CLI
  value wins anyway) and the preset-only `load-on-startup` /
  `stop-timeout` / `dedup-cache-models` (no `--help` counterpart; they are
  typed fields). Both lists live in `router.rs`, and `RouterEditor` hides
  them from the picker.
- **CRITICAL — always poll with `autoload=false`**: a bare
  `GET /slots?model=X` (or `/metrics?model=X`) **loads the model**. With
  `&autoload=false` an unloaded model answers `400 model is not loaded`
  (no side effect) and a `sleeping` one still answers 200 (idle slots,
  zeroed counters). Polling without it would load every registered model
  and defeat router mode entirely.
- **Per-model state**: `StatsState.track` / `.avg` are `HashMap<alias, _>`;
  `poll_model` is the whole live algorithm applied per model, `poll_activity`
  only orchestrates (all HTTP **before** any lock — guards must never be
  held across `.await`). History rows carry `model`; `hist_row_mut` matches
  on `at` **and** model so the catch-up never amends another model's row.
- **Log attribution**: task ids restart per instance, so `print_timing` lines
  are attributed by the `[<port>]` prefix (`lookup_log_timings(path, task,
  port)`); `parse_model_ports` builds alias → instance port from
  `proxying request to model <alias> on port <p>`. Instance lines go to
  **stdout**, router lines to **stderr** (the app merges both into
  `logs/llama-server.log`). Auto-selection = a decoding model, else the last
  one that served a request.
- **Model statuses** (from `GET /models`): `unloaded`, `loading`, `loaded`,
  `sleeping`. `--sleep-idle-seconds` unloads **per instance** (measured);
  `--models-max` evicts LRU (default 4).
- UI: the **Router tab** (`RouterPage` + `RouterEditor`) is the single place
  for routing — it lists **every** downloaded model with a checkbox (ticked
  = exposed, with an editable alias + parameters) plus `models_max` /
  `sleep_idle_seconds`. The Server tab keeps Simple + Expert only and shows
  a `server.routerNote` banner while the router is active; the Models tab
  only downloads/deletes, and its "Utiliser" button appears only when **no**
  model is routed (single-model fallback). Stats tab shows model chips + a
  **Modèle** history column, topbar shows a status chip per model.
- Changing the registry rewrites the INI; the running server keeps the old
  preset until it is restarted (no `?reload=1` in the UI yet).

- **Windows autostart**: `get_autostart`/`set_autostart` commands (`lib.rs`,
  `ManagerExt::autolaunch`), checkbox in the Settings tab (`SettingsPage`); the plugin is
  configured with `--minimized` (hidden-to-tray start, handled in `setup`).
- **Models**: `ModelsPage` takes `selected` (single-model fallback) and
  `hasRouter` → "in use" badge and "Utiliser" only appear when no model is
  routed. Table actions stay on one line (`flex-wrap: nowrap`, see
  `.cell-actions`).
- **Backend page**: installed-backends table first (matches Models layout),
  `BackendPage` takes `preset`/`extraArgs`/`status`/`setServer` from `App` so
  switching default restarts a running server on the new binary with the
  current preset (stop-only if no model configured).
- **Logs**: one-line health (full JSON kept for diag), `.log` fitted to viewport.
- **Release**: `npm run tauri build` (tsc + vite + **perMachine** NSIS → admin needed)
  to `src-tauri/target/release/bundle/nsis/*.exe`; LTO release profile, long
  compile (~10-30 min). `cargo check` is warning-free — keep it that way.

## Conventions / observed pitfalls (do not reintroduce)

1. **Tauri v2 = camelCase invoke args**: the macro generates `rename_all = "camelCase"`
   (`download_url` → `downloadUrl`, `extra_args` → `extraArgs`). Nested struct
   fields (`SimplePreset.model_path`) stay snake_case (direct serde).
2. **`llama-server --help` format**: **column-0** options (`-t, --threads N  desc`,
   multiple aliases, `N`/`START END`/`KEY=TYPE:VALUE,...`/`<..>`/`{a,b}`/`[a|b]` values),
   descriptions continued on **indented lines** (incl. `(env: …)`), `----- xxx -----`
   sections. Never treat an indented line as a section or option.
   The parser is tested on a real excerpt + validated on a full `--help`
   (4 sections, 251 options). `parse_help` strips a possible BOM.
3. **Tray created in code only** (`TrayIconBuilder`): do not add `app.trayIcon`
   to `tauri.conf.json` (double icon otherwise).
4. **Rust `"\` at end of line eats ALL following indentation**: in multiline test
   fixtures, use real newlines (no `\`).
5. `cargo`/`tsc` output goes to stderr: PowerShell shows `NativeCommandError` even on
   success — judge by `test result: ok` / `Finished`, not by pipe exit code.
6. Frontend: no Tailwind (handmade CSS `src/styles.css`, "native utility" style,
   light/dark via `prefers-color-scheme`). Windows case-insensitive files:
   do not create `App.css` + `app.css` (same file on Windows).
7. **NEVER mutate a ref/state inside a `setState` updater**: StrictMode replays
   updaters in dev → emptied history while the backend recorded (`seenKeys`
   inside `setHistory`, Oct 2026). Filter BEFORE, keep the updater pure.
8. **Every dev rebuild restarts the app**: backend memory + React state wiped.
   Do not read a counter restarting at 0 as a bug. HMR can also desync hook/page
   (`undefined` props) → clean restart in that case.
9. `poll_activity` takes `AppHandle` as 1st param (injected by Tauri, invisible to
   the frontend) to resolve the log path. Frontend `invoke` unchanged.
9b. **Never hold a `MutexGuard` across `.await`** in a Tauri command: the
   future stops being `Send` and the build fails with
   `future cannot be sent between threads safely`. `poll_activity` does every
   HTTP call first, then takes the locks for a purely synchronous section.
10. `shell` tool: commands with `$env:…` or quotes must go through a `.ps1` file
    (via `write`) run with `powershell -NoProfile -ExecutionPolicy Bypass -File …` —
    inline escaping breaks (e.g. `Select-String` with `|`/`\k`, `-split`/`-match`
    with `"…"`). `Get-Content …log | Select-String`: prefer
    `Where-Object { $_ -match '…' }` in a script, or `findstr`.
11. **Icons**: `src-tauri/icons/icon.ico` is linked into the exe via `resource.rc`
    generated by `tauri-build`, but the build script does **not** rerun when the
    `.ico` changes (only `.rs` triggers it). After `npx tauri icon …`: `cargo clean -p
    bonsaicpp-manager` to force `resource.lib` regeneration, otherwise the exe keeps
    the old icon. Check: `[Drawing.Icon]::ExtractAssociatedIcon($exe)`.
    The Windows icon cache may also show a stale icon (taskbar/tray).
12. **`--help` parser**: only `N` (llama convention) yields `Int` — any other
    uppercase value name (`TYPE`, `STRING`…) is `Text` (a number field breaks input,
    e.g. `--cache-type-k TYPE` expects `f16`). A numeric default already decides
    int/float when present. Bare comma-separated lowercase lists
    (`--spec-type none,draft-simple,…,ngram-cache`) are `Enum`; uppercase
    placeholders (`N0,N1,…`) and `<…>` patterns (`<dev1,dev2,..>`) stay `Text`.
    Very long value lists wrap onto a column-0 continuation line starting with
    `,` — it is appended to the pending value, not dropped (otherwise the flag
    degrades to `Bool`).
13. **Windows child processes must not flash a console**: every
    `std/tokio::process::Command` (`nvidia-smi`, `powershell` WMI probe,
    `llama-server --help`, `netstat`, `tasklist`, `taskkill`) needs
    `creation_flags(0x08000000 /* CREATE_NO_WINDOW */)` via
    `std::os::windows::process::CommandExt` (cfg-gated). The release binary is
    `windows_subsystem` (no console), so any child without the flag pops a
    visible terminal — invisible in `tauri dev` (console subsystem, inherited).
    The `llama-server` spawn itself uses `DETACHED_PROCESS | CREATE_NO_WINDOW`.
14. **GPU VRAM (topbar)**: `gpu::gpu_memory` command. Adapter identity + totals
    via DXGI (in-process, all vendors incl. AMD where `rocm-smi` doesn't exist
    on Windows); **usage via PDH `GPU Adapter Memory(*)\Dedicated Usage`**
    (English counter names, locale-independent) — DXGI `CurrentUsage` is
    per-process (our own ~0), NOT adapter-wide. Totals summed over adapters;
    richest one named (WARP skipped). Polled every 5 s with `serverStatus`
    (`App.tsx`), hidden if the query fails.
15. **Tray icon state**: colored while the server runs, gray when stopped
    (`tray_icon::refresh_tray_icon`, `TrayIconState`). The gray variant is
    derived at runtime from the app icon (luma) — no second asset. Updated in
    `start_server` / `stop_server_impl` / every `server_status` return (covers
    crash + orphan adoption); no redundant `set_icon` (polled every 5 s).
    State is tri-state (`None` = unknown) so the launch-time gray applies.
16. **Server autostart at app launch**: `bonsai.autostartServer` localStorage
    flag, checkbox in the Settings tab (`SettingsPage`), acted on once at mount
    (`App.tsx`, `autoStarted` ref for StrictMode) — skipped if a server is
    already running (orphan) or no model is configured.
17. **i18n (FR default, EN)**: frontend via `react-i18next`
    (`src/i18n/fr.json` reference, `en.json`, `index.ts`; init imported in
    `main.tsx`). Language resolution: `localStorage "bonsai.locale"`, else
    `navigator.language` (EN OS → EN), else FR; `<html lang>` synced.
    Switcher in the Settings tab (`SettingsPage`, `<select>`) persists + pushes to the
    backend (`set_app_language`, tray menu relabel). `npm run i18n:check`
    (`scripts/i18n-check.mjs`) fails on key mismatch — run it when touching
    strings. New language = copy `fr.json`, register in `resources` +
    `supportedLngs` + `LOCALES`. Number/date formatting via
    `numLocale()` (`api.ts`), never hardcoded `fr-FR`. French uses proper
    accents. Backend (`src-tauri/src/locale.rs`): `Lang`/`AppLang` state,
    `tray_labels`, `Msg` table for every user-facing error (technical `{e}`
    suffixes stay raw); commands take injected `AppHandle` to resolve the
    lang (invisible to the frontend); persisted `lang.txt`, else OS locale
    (`sys-locale`) at startup. Tray tooltip is frontend-built (`useStats`):
    decode active → speeds, else `tray.ready` ("Bonsai prêt").
18. **Sidebar order** (`App.tsx`): Backend → Serveur → Modèles → Routeur →
    Statistiques → Logs → Paramètres → À propos (dependency order: the
    backend provides the binary, the server runs it, then the models it can
    serve, then how they are routed).
19. **Settings tab** (`SettingsPage`): **app** preferences only — autostart
    at login, server autostart at launch, language (`<select>` over
    `LOCALES`, extensible), stats polling interval (`bonsai.pollMs`, idle
    only; 0.5 s while generating), tray tooltip toggle (`bonsai.tooltip`).
    Router knobs (`models_max`, `sleep_idle_seconds`) live on the **Router**
    tab, not here. Nothing settings-like lives in the sidebar footer
    (removed), the Server tab, or the Stats tab.
20. **Commits**: messages in English, with an `openchamber` co-author trailer:
    `Co-authored-by: OpenChamber`. Applies from `a00f998` onward (earlier
    commits predate the rule and are left untouched).
