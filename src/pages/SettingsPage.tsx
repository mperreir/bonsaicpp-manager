import { useTranslation } from "react-i18next";
import i18n, { LOCALES, persistLocale, type AppLocale } from "../i18n";
import { api, numLocale, type RouterConfig } from "../lib/api";

interface Props {
  autostart: boolean | null;
  onToggleAutostart: () => void;
  autoStartServer: boolean;
  setAutoStartServer: (v: boolean) => void;
  pollMs: number;
  setPollMs: (ms: number) => void;
  tooltipOn: boolean;
  setTooltipOn: (v: boolean) => void;
  router: RouterConfig;
  setRouter: (c: RouterConfig) => void;
}

const POLL_OPTIONS = [500, 1000, 2000, 5000];
const MODELS_MAX = [1, 2, 3, 4, 6, 8, 0];
const SLEEP_IDLE = [-1, 60, 300, 900, 1800];

export default function SettingsPage({
  autostart, onToggleAutostart,
  autoStartServer, setAutoStartServer,
  pollMs, setPollMs,
  tooltipOn, setTooltipOn,
  router, setRouter,
}: Props) {
  const { t } = useTranslation();
  const loc = numLocale();
  const current: AppLocale = i18n.language.startsWith("en") ? "en" : "fr";

  const changeLang = (lng: AppLocale) => {
    persistLocale(lng);
    void i18n.changeLanguage(lng);
    // The tray menu is backend-owned: keep it in sync.
    api.setAppLanguage(lng).catch(() => {});
  };

  return (
    <div className="page">
      <h2>{t("settings.title")}</h2>

      <section className="card">
        <h3>{t("settings.startup")}</h3>
        <label className="row">
          <input
            type="checkbox"
            checked={autostart ?? false}
            disabled={autostart == null}
            onChange={onToggleAutostart}
          />
          {t("settings.appAutostart")}
        </label>
        <label className="row">
          <input
            type="checkbox"
            checked={autoStartServer}
            onChange={(e) => setAutoStartServer(e.target.checked)}
          />
          {t("settings.serverAutostart")}
        </label>
      </section>

      <section className="card">
        <h3>{t("settings.interface")}</h3>
        <label className="row">{t("settings.language")}
          <select value={current} onChange={(e) => changeLang(e.target.value as AppLocale)}>
            {LOCALES.map((l) => (
              <option key={l.id} value={l.id}>{l.label}</option>
            ))}
          </select>
        </label>
      </section>

      <section className="card">
        <h3>{t("settings.stats")}</h3>
        <label className="row">{t("settings.pollFreq")}
          <select value={pollMs} onChange={(e) => setPollMs(Number(e.target.value))}>
            {POLL_OPTIONS.map((v) => (
              <option key={v} value={v}>
                {(v / 1000).toLocaleString(loc)} s
              </option>
            ))}
          </select>
          <span className="muted small">{t("settings.pollHelp")}</span>
        </label>
        <label className="row">
          <input
            type="checkbox"
            checked={tooltipOn}
            onChange={(e) => setTooltipOn(e.target.checked)}
          />
          {t("settings.tooltip")}
        </label>
      </section>

      {router.models.length > 0 && (
        <section className="card">
          <h3>{t("router.routing")}</h3>
          <label className="row">{t("router.modelsMax")}
            <select value={router.models_max} onChange={(e) => setRouter({ ...router, models_max: Number(e.target.value) })}>
              {MODELS_MAX.map((v) => (
                <option key={v} value={v}>
                  {v === 0 ? t("router.unlimited") : `${v}`}
                </option>
              ))}
            </select>
            <span className="muted small">{t("router.modelsMaxHelp")}</span>
          </label>
          <label className="row">{t("router.sleepIdle")}
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
