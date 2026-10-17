import { useTranslation } from "react-i18next";
import i18n, { LOCALES, persistLocale, type AppLocale } from "../i18n";
import { api, numLocale } from "../lib/api";

interface Props {
  autostart: boolean | null;
  onToggleAutostart: () => void;
  autoStartServer: boolean;
  setAutoStartServer: (v: boolean) => void;
  pollMs: number;
  setPollMs: (ms: number) => void;
  tooltipOn: boolean;
  setTooltipOn: (v: boolean) => void;
}

const POLL_OPTIONS = [500, 1000, 2000, 5000];

export default function SettingsPage({
  autostart, onToggleAutostart,
  autoStartServer, setAutoStartServer,
  pollMs, setPollMs,
  tooltipOn, setTooltipOn,
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

    </div>
  );
}
