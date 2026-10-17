import { invoke } from "@tauri-apps/api/core";
import i18n from "../i18n";

/// BCP-47 tag for number/date formatting matching the UI language.
export function numLocale(): string {
  return i18n.language?.startsWith("en") ? "en-US" : "fr-FR";
}

export interface BackendStatus {
  installed: boolean;
  tag: string | null;
  flavor: string | null;
  server_exe: string | null;
  size_bytes: number | null;
  suggested_flavor: string;
}

export interface InstalledBackend {
  tag: string;
  flavor: string;
  server_exe: string;
  size_bytes: number;
  is_default: boolean;
}

export interface AssetInfo {
  name: string;
  size: number;
  download_url: string;
  flavor: string;
}

export interface ReleaseInfo {
  tag: string;
  name: string;
  published_at: string | null;
  assets: AssetInfo[];
}

export interface HelpOption {
  flag: string;
  aliases: string[];
  value_name: string | null;
  description: string;
  default: string | null;
  kind: { [k: string]: unknown } | string;
  section: string;
}

export interface HelpSchema {
  sections: string[];
  options: HelpOption[];
  raw_version: string | null;
  raw: string;
  parsed_at: string;
}

export interface GpuMemory {
  used_bytes: number;
  total_bytes: number;
  budget_bytes: number;
  adapter: string;
}

export interface AboutInfo {
  version: string;
  built_at: string;
}

export interface LocalModel {
  file: string;
  size_bytes: number;
  repo: string;
  quant: string;
}

export interface RemoteFile {
  name: string;
  size: number | null;
  recommended: boolean;
  universal: boolean;
}

export interface ServerStatus {
  running: boolean;
  port: number | null;
  model: string | null;
  pid: number | null;
  uptime_secs: number | null;
  cmdline: string | null;
}

export interface SimplePreset {
  model_path: string;
  ctx: number;
  ngl: number;
  threads: number;
  host: string;
  port: number;
  reasoning_effort: string;
  mmproj_path: string | null;
}

export interface PromptMetrics {
  prompt_tps: number | null;
  predicted_tps: number | null;
  requests_processing: number | null;
  requests_deferred: number | null;
  tokens_cache_used: number | null;
  raw_scrape_ok: boolean;
}

export interface SlotLive {
  id: number;
  phase: string;
  prompt_done: number;
  prompt_total: number;
  decoded: number;
  live_tps: number | null;
}

export interface Activity {
  router: boolean;
  models: ModelActivity[];
  selected_model: string | null;
  prefill_tps: number | null;
  decode_tps: number | null;
  avg_prefill_tps: number | null;
  avg_decode_tps: number | null;
  avg_prefill_polls: number;
  avg_decode_polls: number;
  cache_live: number | null;
  cache_session: number | null;
  cache_session_cached: number;
  cache_session_total: number;
  spec_accept: number | null;
  spec_accepted: number;
  spec_drafted: number;
  ttft_last: number | null;
  ttft_avg: number | null;
  ttft_n: number;
  active_slots: SlotLive[];
  ctx_used: number;
  ctx_max: number | null;
  requests_processing: number | null;
  requests_deferred: number | null;
  history: TimingSample[];
}

export interface ModelActivity {
  alias: string;
  status: string;
  active: boolean;
  prefill_tps: number | null;
  decode_tps: number | null;
  ctx_used: number;
  ctx_max: number | null;
}

export interface TimingSample {
  at: string;
  model: string | null;
  prompt_n: number | null;
  prompt_ms: number | null;
  prompt_tps: number | null;
  cache_n: number | null;
  cache_total_n: number | null;
  predicted_n: number | null;
  predicted_ms: number | null;
  predicted_tps: number | null;
  spec_drafted: number | null;
  spec_accepted: number | null;
}

/// One model registered in the router (an INI section of the preset).
export interface RouterModel {
  alias: string;
  model_path: string;
  /// Per-model overrides, `[key, value]` pairs (empty value = bare flag).
  overrides: [string, string][];
  load_on_startup: boolean;
}

export interface RouterConfig {
  models: RouterModel[];
  /// `[*]` section: defaults shared by every model.
  global: [string, string][];
  models_max: number;
  sleep_idle_seconds: number;
}

export type IniErrorKind = "unknownKey" | "routerControlled" | "presetOnly";

export interface IniError {
  scope: string;
  key: string;
  reason: IniErrorKind;
}

export const EMPTY_ROUTER: RouterConfig = {
  models: [],
  global: [],
  models_max: 4,
  sleep_idle_seconds: -1,
};

/// Suggested alias for a GGUF file: short, lowercase, no size/quant noise.
export function suggestAlias(file: string): string {
  const base = file.split(/[\\/]/).pop() ?? file;
  return (
    base
      .replace(/\.gguf$/i, "")
      .replace(/-?\d+(\.\d+)?\s*b\b/gi, "")   // 27B, 9B
      .replace(/[-_](pq|q)\d[\w.]*$/i, "")      // -PQ2_0, -Q8_0
      .replace(/[-_](mtp|abliterated|swift-cap|chat|it|instruct)$/i, "")
      .replace(/[-_]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .toLowerCase()
      .slice(0, 40) || "model"
  );
}

export function fmtSpec(drafted: number | null | undefined, accepted: number | null | undefined): string {
  if (drafted == null || accepted == null || drafted <= 0) return "—";
  return `${((accepted / drafted) * 100).toFixed(0)} %`;
}

export const DEFAULT_REPO = "prism-ml/Ternary-Bonsai-2-27B-gguf";

export const api = {
  detectGpu: () => invoke<string>("detect_gpu"),
  backendStatus: () => invoke<BackendStatus>("backend_status"),
  listReleases: (limit = 5) => invoke<ReleaseInfo[]>("list_backend_releases", { limit }),
  installBackend: (tag: string, flavor: string, downloadUrl: string) =>
    invoke<BackendStatus>("install_backend", { tag, flavor, downloadUrl }),
  listInstalledBackends: () => invoke<InstalledBackend[]>("list_installed_backends"),
  setDefaultBackend: (tag: string, flavor: string) =>
    invoke<InstalledBackend[]>("set_default_backend", { tag, flavor }),
  deleteBackend: (tag: string, flavor: string) =>
    invoke<InstalledBackend[]>("delete_backend", { tag, flavor }),
  parseHelp: () => invoke<HelpSchema>("parse_server_help"),
  listLocalModels: () => invoke<LocalModel[]>("list_local_models"),
  listRemoteFiles: (repo: string, token?: string) =>
    invoke<RemoteFile[]>("list_remote_files", { repo, token: token ?? null }),
  downloadModel: (repo: string, filename: string, token?: string) =>
    invoke<string>("download_model_file", { repo, filename, token: token ?? null }),
  deleteModel: (path: string) => invoke<void>("delete_model_file", { path }),
  buildCmdline: (preset: SimplePreset, extraArgs: Record<string, string>) =>
    invoke<string[]>("build_cmdline", { preset, extraArgs }),
  startServer: (preset: SimplePreset, extraArgs: Record<string, string>, router: RouterConfig) =>
    invoke<ServerStatus>("start_server", { preset, extraArgs, router }),
  routerConfig: () => invoke<RouterConfig>("router_config"),
  routerValidate: (config: RouterConfig) => invoke<IniError[]>("router_validate", { config }),
  setRouterConfig: (config: RouterConfig) => invoke<IniError[]>("set_router_config", { config }),
  stopServer: () => invoke<ServerStatus>("stop_server"),
  serverStatus: () => invoke<ServerStatus>("server_status"),
  serverHealth: (port: number) => invoke<unknown>("server_health", { port }),
  serverLogTail: (lines = 200) => invoke<string>("server_log_tail", { lines }),
  scrapeMetrics: (port: number) => invoke<PromptMetrics>("scrape_metrics", { port }),
  pollActivity: (port: number) => invoke<Activity>("poll_activity", { port }),
  gpuMemory: () => invoke<GpuMemory>("gpu_memory"),
  aboutInfo: () => invoke<AboutInfo>("about_info"),
  recordTiming: (t: { prompt_n?: number | null; prompt_ms?: number | null; predicted_n?: number | null; predicted_ms?: number | null }) =>
    invoke<TimingSample>("record_timing", {
      promptN: t.prompt_n ?? null,
      promptMs: t.prompt_ms ?? null,
      predictedN: t.predicted_n ?? null,
      predictedMs: t.predicted_ms ?? null,
    }),
  sendTestPrompt: (port: number) => invoke<TimingSample>("send_test_prompt", { port }),
  setTrayTooltip: (text: string) => invoke<void>("set_tray_tooltip", { text }),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (on: boolean) => invoke<boolean>("set_autostart", { on }),
  setAppLanguage: (lang: string) => invoke<string>("set_app_language", { lang }),
};

export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  if (n < 1024) return `${n} ${numLocale() === "en-US" ? "B" : "o"}`;
  const units = numLocale() === "en-US"
    ? ["B", "KB", "MB", "GB", "TB"]
    : ["o", "Ko", "Mo", "Go", "To"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
}

export function fmtTps(v: number | null | undefined): string {
  if (v == null || !isFinite(v)) return "—";
  return `${v.toFixed(1)} t/s`;
}

export function fmtPct(v: number | null | undefined): string {
  if (v == null || !isFinite(v)) return "—";
  return `${(v * 100).toFixed(1)} %`;
}

export function fmtPctRatio(cached: number | null | undefined, total: number | null | undefined): string {
  if (cached == null || total == null || total <= 0) return "—";
  return fmtPct(Math.min(1, Math.max(0, cached / total)));
}

export function fmtSecs(v: number | null | undefined): string {
  if (v == null || !isFinite(v)) return "—";
  return `${v.toFixed(2)} s`;
}

/// Effective host the server listens on: an expert `--host` override wins
/// over the simple preset (extra args are appended last on the cmdline).
export function serverHost(preset: SimplePreset, extraArgs: Record<string, string>): string {
  const override = extraArgs["--host"]?.trim();
  return override ? override : preset.host;
}
