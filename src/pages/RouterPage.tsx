import { useTranslation } from "react-i18next";
import { numLocale, type IniError, type RouterConfig } from "../lib/api";
import RouterEditor from "./RouterEditor";

interface Props {
  router: RouterConfig;
  setRouter: (c: RouterConfig) => void;
  saveRouter: (c: RouterConfig) => Promise<IniError[]>;
}

const MODELS_MAX = [1, 2, 3, 4, 6, 8, 0];
const SLEEP_IDLE = [-1, 60, 300, 900, 1800];

/// Models exposed by the server: which GGUF is reachable, under which
/// alias, with which parameters. The routing knobs that govern how many
/// stay resident live here too.
export default function RouterPage({ router, setRouter, saveRouter }: Props) {
  const { t } = useTranslation();
  const loc = numLocale();
  const routedCount = router.models.filter((m) => m.routed).length;

  return (
    <div className="page">
      <h2>{t("router.pageTitle")}</h2>
      <p className="muted small">{t("router.pageIntro")}</p>

      <RouterEditor router={router} setRouter={setRouter} saveRouter={saveRouter} />

      {routedCount > 0 && (
        <section className="card">
          <h3>{t("router.routing")}</h3>
          <label className="row">
            {t("router.modelsMax")}
            <select value={router.models_max} onChange={(e) => setRouter({ ...router, models_max: Number(e.target.value) })}>
              {MODELS_MAX.map((v) => (
                <option key={v} value={v}>{v === 0 ? t("router.unlimited") : `${v}`}</option>
              ))}
            </select>
            <span className="muted small">{t("router.modelsMaxHelp")}</span>
          </label>
          <label className="row">
            {t("router.sleepIdle")}
            <select value={router.sleep_idle_seconds} onChange={(e) => setRouter({ ...router, sleep_idle_seconds: Number(e.target.value) })}>
              {SLEEP_IDLE.map((v) => (
                <option key={v} value={v}>
                  {v < 0 ? t("router.never") : `${(v / 60).toLocaleString(loc)} min`}
                </option>
              ))}
            </select>
            <span className="muted small">{t("router.sleepIdleHelp")}</span>
          </label>
        </section>
      )}
    </div>
  );
}
