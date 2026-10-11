import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type ServerStatus } from "../lib/api";

export default function LogsPage({ status }: { status: ServerStatus }) {
  const { t } = useTranslation();
  const [log, setLog] = useState("");
  const [healthRaw, setHealthRaw] = useState("");
  const [auto, setAuto] = useState(true);
  const [error, setError] = useState("");

  const openServer = async () => {
    if (!(status.running && status.port)) return;
    const url = `http://localhost:${status.port}`;
    try {
      await openUrl(url);
    } catch (e) {
      console.error(e);
      alert(t("errors.openBrowser", { url }));
    }
  };
  const refresh = async () => {
    try {
      setLog(await api.serverLogTail(200));
    } catch (e) {
      setError(String(e));
    }
    if (status.running && status.port) {
      try {
        const h = await api.serverHealth(status.port);
        setHealthRaw(JSON.stringify(h));
      } catch (e) {
        setHealthRaw(t("logs.unreachable", { error: String(e) }));
      }
    } else {
      setHealthRaw("");
    }
  };

  useEffect(() => {
    refresh();
    if (!auto) return;
    const id = setInterval(refresh, 3000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, status.running, status.port]);

  const copyDiag = async () => {
    const diag = `Bonsai.cpp Manager diag — ${new Date().toISOString()}\nrunning=${status.running} port=${status.port} pid=${status.pid}\nhealth=${healthRaw}\n--- log tail ---\n${log}\n`;
    await navigator.clipboard.writeText(diag);
    alert(t("logs.diagCopied"));
  };

  // Compact one-line health (the full JSON stays in the diag).
  let healthShort = "";
  if (status.running && healthRaw) {
    try {
      const st = (JSON.parse(healthRaw) as { status?: string }).status;
      healthShort = st ? `● ${st} • :${status.port}` : `● :${status.port} • ${healthRaw.slice(0, 80)}`;
    } catch {
      healthShort = `● ${healthRaw.slice(0, 80)}`;
    }
  }

  return (
    <div className="page fill">
      <h2>{t("logs.title")}</h2>
      {error && <div className="alert error">{error}</div>}
      <div className="row">
        <button onClick={refresh}>{t("common.refresh")}</button>
        <label><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> {t("logs.auto")}</label>
        {status.running && status.port && (
          <button onClick={openServer} title={`http://localhost:${status.port}`}>{t("logs.openWeb")}</button>
        )}
        <button onClick={copyDiag}>{t("logs.export")}</button>
        {healthShort && <span className="muted small">{healthShort}</span>}
      </div>
      <section className="card fill">
        <h3>{t("logs.logTitle")}</h3>
        <pre className="log fill">{log || t("logs.empty")}</pre>
      </section>
    </div>
  );
}
