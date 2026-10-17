import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import i18n from "./i18n";
import { api, EMPTY_ROUTER, fmtBytes, serverHost, suggestAlias, type GpuMemory, type IniError, type RouterConfig, type ServerStatus, type SimplePreset } from "./lib/api";
import BackendPage from "./pages/BackendPage";
import ModelsPage from "./pages/ModelsPage";
import ServerPage from "./pages/ServerPage";
import SettingsPage from "./pages/SettingsPage";
import AboutPage from "./pages/AboutPage";
import StatsPage from "./pages/StatsPage";
import LogsPage from "./pages/LogsPage";
import { useStats } from "./hooks/useStats";
import "./styles.css";

type Tab = "backend" | "modeles" | "serveur" | "stats" | "logs" | "parametres" | "apropos";

const PRESET_KEY = "bonsai.preset";
const EXTRA_KEY = "bonsai.extraArgs";
const AUTO_START_SERVER_KEY = "bonsai.autostartServer";
const ROUTER_KEY = "bonsai.router";

/// The router registry is owned by the backend (it writes the preset);
/// localStorage only mirrors it so the UI has something to show offline.
function loadRouter(): RouterConfig {
  try {
    const raw = localStorage.getItem(ROUTER_KEY);
    if (raw) return { ...EMPTY_ROUTER, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return EMPTY_ROUTER;
}

function loadPreset(): SimplePreset {
  try {
    const raw = localStorage.getItem(PRESET_KEY);
    if (raw) return { ...defaultPreset(), ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return defaultPreset();
}

function defaultPreset(): SimplePreset {
  return {
    model_path: "",
    ctx: 0,
    ngl: -1,
    threads: 0,
    host: "127.0.0.1",
    port: 8080,
    reasoning_effort: "xhigh",
    mmproj_path: null,
  };
}

function loadExtraArgs(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(EXTRA_KEY) ?? "{}");
  } catch {
    return {};
  }
}

export default function App() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("backend");
  const [status, setStatus] = useState<ServerStatus>({ running: false, port: null, model: null, pid: null, uptime_secs: null, cmdline: null });
  const [gpu, setGpu] = useState<GpuMemory | null>(null);
  const [preset, setPresetState] = useState<SimplePreset>(loadPreset);
  const [extraArgs, setExtraArgsState] = useState<Record<string, string>>(loadExtraArgs);
  const [router, setRouterState] = useState<RouterConfig>(loadRouter);
  const [autoStartServer, setAutoStartServerState] = useState<boolean>(() => {
    try {
      return localStorage.getItem(AUTO_START_SERVER_KEY) === "1";
    } catch {
      return false;
    }
  });
  const autoStarted = useRef(false);

  const setPreset = (p: SimplePreset) => {
    setPresetState(p);
    localStorage.setItem(PRESET_KEY, JSON.stringify(p));
  };
  const setExtraArgs = (e: Record<string, string>) => {
    setExtraArgsState(e);
    localStorage.setItem(EXTRA_KEY, JSON.stringify(e));
  };
  const setAutoStartServer = (v: boolean) => {
    setAutoStartServerState(v);
    try {
      localStorage.setItem(AUTO_START_SERVER_KEY, v ? "1" : "0");
    } catch {
      /* ignore */
    }
  };
  /// Validate against the live `--help`, then persist (backend writes the
  /// INI). Returns the rejected keys; nothing is saved when non-empty.
  const saveRouter = async (cfg: RouterConfig): Promise<IniError[]> => {
    const errs = await api.setRouterConfig(cfg);
    if (errs.length === 0) {
      setRouterState(cfg);
      try {
        localStorage.setItem(ROUTER_KEY, JSON.stringify(cfg));
      } catch {
        /* ignore */
      }
    }
    return errs;
  };
  const setRouter = (cfg: RouterConfig) => {
    setRouterState(cfg);
    try {
      localStorage.setItem(ROUTER_KEY, JSON.stringify(cfg));
    } catch {
      /* ignore */
    }
  };
  /// Register a downloaded GGUF in the router from the Models tab.
  const addToRouter = async (file: string) => {
    let alias = suggestAlias(file);
    let n = 2;
    while (router.models.some((m) => m.alias === alias)) alias = `${suggestAlias(file)}-${n++}`;
    const errs = await saveRouter({
      ...router,
      models: [...router.models, { alias, model_path: file, overrides: [], load_on_startup: false }],
    });
    if (errs.length === 0) setTab("serveur");
  };

  const stats = useStats(status);
  const [autostart, setAutostartState] = useState<boolean | null>(null);

  useEffect(() => {
    // Converge the backend (tray menu) on the resolved UI language.
    api.setAppLanguage(i18n.language).catch(() => {});
    // The backend owns the registry (it regenerates the preset from it).
    api.routerConfig().then((cfg) => {
      setRouterState({ ...EMPTY_ROUTER, ...cfg });
      try {
        localStorage.setItem(ROUTER_KEY, JSON.stringify(cfg));
      } catch {
        /* ignore */
      }
    }).catch(() => {});
    api.serverStatus().then((st) => {
      setStatus(st);
      // Launch-time server autostart (once, incl. StrictMode double-mount):
      // skip if a server is already there (adopted orphan).
      if (!autoStarted.current) {
        autoStarted.current = true;
        let flag = false;
        try {
          flag = localStorage.getItem(AUTO_START_SERVER_KEY) === "1";
        } catch {
          /* ignore */
        }
        if (flag && !st.running) {
          const p = loadPreset();
          const r = loadRouter();
          // Router mode needs no model at launch; single-model mode does.
          if (r.models.length > 0 || p.model_path) {
            api.startServer(p, loadExtraArgs(), r).then(setStatus).catch(() => {});
          }
        }
      }
    }).catch(() => {});
    api.gpuMemory().then(setGpu).catch(() => setGpu(null));
    api.getAutostart().then(setAutostartState).catch(() => setAutostartState(false));
    const id = setInterval(() => {
      api.serverStatus().then(setStatus).catch(() => {});
      api.gpuMemory().then(setGpu).catch(() => setGpu(null));
    }, 5000);
    return () => clearInterval(id);
  }, []);

  const toggleAutostart = async () => {
    if (autostart == null) return;
    try {
      setAutostartState(await api.setAutostart(!autostart));
    } catch (e) {
      console.error(e);
    }
  };

  const toggleServer = async () => {
    if (status.running) {
      setStatus(await api.stopServer());
    } else if (router.models.length > 0) {
      // Router mode: nothing to load, models come with the API calls.
      setStatus(await api.startServer(preset, extraArgs, router));
    } else if (preset.model_path) {
      setStatus(await api.startServer(preset, extraArgs, EMPTY_ROUTER));
    } else {
      setTab("modeles");
    }
  };
  const hasRouter = router.models.length > 0;

  const openHost = preset.host === "0.0.0.0" ? "localhost" : preset.host;
  const serverUrl =
    status.running && status.port ? `http://${openHost}:${status.port}` : null;
  const openServer = async () => {
    if (!serverUrl) return;
    try {
      await openUrl(serverUrl);
    } catch (e) {
      console.error(e);
      alert(t("errors.openBrowser", { url: serverUrl }));
    }
  };

  return (
    <div className="shell">
      <aside className="side">
        <div className="brand">Bonsai.cpp Manager</div>
        <nav>
          {[["backend", t("nav.backend")], ["modeles", t("nav.models")], ["serveur", t("nav.server")], ["stats", t("nav.stats")], ["logs", t("nav.logs")], ["parametres", t("nav.settings")], ["apropos", t("nav.about")]].map(([id, label]) => (
            <button key={id as Tab} className={tab === id ? "active" : ""} onClick={() => setTab(id as Tab)}>{label}</button>
          ))}
        </nav>
      </aside>
      <div className="main">
        <header className="topbar">
          <span className={`dot ${status.running ? "on" : "off"}`} title={status.running ? t("topbar.runningTitle") : t("topbar.stoppedTitle")} />
          <span className="muted">{status.running ? `${serverHost(preset, extraArgs)}:${status.port} • pid ${status.pid}` : t("topbar.stopped")}</span>
          {status.running && hasRouter && (stats.activity?.models ?? []).map((m) => (
            <span
              key={m.alias}
              className={`chip m-${m.status} ${m.active ? "busy" : ""}`}
              title={t(`router.status.${m.status}`, { alias: m.alias })}
            >
              {m.alias}
            </span>
          ))}
          {gpu && gpu.total_bytes > 0 && (
            <span className="muted" title={`${gpu.adapter} — budget ${fmtBytes(gpu.budget_bytes)}`}>
              • GPU {fmtBytes(gpu.used_bytes)} / {fmtBytes(gpu.total_bytes)}
            </span>
          )}
          <span className="spacer" />
          {serverUrl && (
            <button onClick={openServer} title={serverUrl}>
              {t("topbar.open", { host: serverUrl.replace("http://", "") })}
            </button>
          )}
          <button onClick={toggleServer} className="primary" disabled={!status.running && !preset.model_path && !hasRouter} title={!status.running && !preset.model_path && !hasRouter ? t("topbar.chooseModelFirst") : ""}>
            {status.running ? t("topbar.stop") : t("topbar.start")}
          </button>
        </header>
        <main className="content">
          {tab === "backend" && <BackendPage preset={preset} extraArgs={extraArgs} status={status} setServer={setStatus} />}
          {tab === "modeles" && <ModelsPage selected={preset.model_path} router={router} onAddToRouter={addToRouter} onSelect={(p) => { setPreset({ ...preset, model_path: p }); setTab("serveur"); }} />}
          {tab === "serveur" && <ServerPage preset={preset} setPreset={setPreset} extraArgs={extraArgs} setExtraArgs={setExtraArgs} status={status} setStatus={setStatus} router={router} saveRouter={saveRouter} setRouter={setRouter} />}
          {tab === "stats" && <StatsPage status={status} stats={stats} />}
          {tab === "logs" && <LogsPage status={status} />}
          {tab === "parametres" && (
            <SettingsPage
              autostart={autostart}
              onToggleAutostart={toggleAutostart}
              autoStartServer={autoStartServer}
              setAutoStartServer={setAutoStartServer}
              pollMs={stats.pollMs}
              setPollMs={stats.setPollMs}
              tooltipOn={stats.tooltipOn}
              setTooltipOn={stats.setTooltipOn}
              router={router}
              setRouter={setRouter}
            />
          )}
          {tab === "apropos" && <AboutPage />}
        </main>
      </div>
    </div>
  );
}
