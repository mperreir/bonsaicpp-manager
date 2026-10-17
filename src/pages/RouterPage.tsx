import { useState } from "react";
import { useTranslation } from "react-i18next";
import { numLocale, type IniError, type RouterConfig, type ServerStatus } from "../lib/api";
import RouterEditor from "./RouterEditor";

interface Props {
  router: RouterConfig;
  setRouter: (c: RouterConfig) => void;
  saveRouter: (c: RouterConfig) => Promise<IniError[]>;
  status: ServerStatus;
  /** Restart the server (stop + start) with the current config. */
  onRestart: () => Promise<void>;
}

const MODELS_MAX = [1, 2, 3, 4, 6, 8, 0];
const SLEEP_IDLE = [-1, 60, 300, 900, 1800];

/// Models exposed by the server: which GGUF is reachable, under which
/// alias, with which parameters.
///
/// Two kinds of setting live here, and they are **not** applied the same
/// way:
/// - everything written in the preset (exposure, aliases, per-model
///   parameters, the `[*]` defaults) is applied to a running server through
///   `GET /models?reload=1` — no restart;
/// - `--models-max` / `--sleep-idle-seconds` are router-owned command-line
///   flags, so they only take effect after a real restart.
export default function RouterPage({ router, setRouter, saveRouter, status, onRestart }: Props) {
  const { t } = useTranslation();
  const loc = numLocale();
  const routedCount = router.models.filter((m) => m.routed).length;
  const [pendingRestart, setPendingRestart] = useState(false);
  const [restarting, setRestarting] = useState(false);

  const setCliSetting = (patch: Partial<RouterConfig>) => {
    setRouter({ ...router, ...patch });
    if (status.running) setPendingRestart(true);
  };

  const restart = async () => {
    setRestarting(true);
    try {
      await onRestart();
      setPendingRestart(false);
    } finally {
      setRestarting(false);
    }
  };

  return (
    <div className="page">
      <h2>{t("router.pageTitle")}</h2>
      <p className="muted small">{t("router.pageIntro")}</p>

      <RouterEditor
        router={router}
        setRouter={setRouter}
        saveRouter={saveRouter}
        serverPort={status.running ? status.port : null}
      />

      {routedCount > 0 && (
        <section className="card">
          <div className="row">
            <h3>{t("router.routing")}</h3>
            <span className="spacer" />
            {pendingRestart && status.running && (
              <button onClick={restart} disabled={restarting} className="primary">
                {restarting ? t("router.restarting") : t("router.restart")}
              </button>
            )}
          </div>
          <label className="row">
            {t("router.modelsMax")}
            <select value={router.models_max} onChange={(e) => setCliSetting({ models_max: Number(e.target.value) })}>
              {MODELS_MAX.map((v) => (
                <option key={v} value={v}>{v === 0 ? t("router.unlimited") : `${v}`}</option>
              ))}
            </select>
            <span className="muted small">{t("router.modelsMaxHelp")}</span>
          </label>
          <label className="row">
            {t("router.sleepIdle")}
            <select value={router.sleep_idle_seconds} onChange={(e) => setCliSetting({ sleep_idle_seconds: Number(e.target.value) })}>
              {SLEEP_IDLE.map((v) => (
                <option key={v} value={v}>
                  {v < 0 ? t("router.never") : `${(v / 60).toLocaleString(loc)} min`}
                </option>
              ))}
            </select>
            <span className="muted small">{t("router.sleepIdleHelp")}</span>
          </label>
          {status.running && (
            <p className={`muted small ${pendingRestart ? "warn" : ""}`}>
              {pendingRestart ? t("router.cliRestartNeeded") : t("router.cliNote")}
            </p>
          )}
        </section>
      )}
    </div>
  );
}
