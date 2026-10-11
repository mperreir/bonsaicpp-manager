import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { api, fmtBytes, type BackendStatus, type InstalledBackend, type ReleaseInfo, type ServerStatus, type SimplePreset } from "../lib/api";

export default function BackendPage({ preset, extraArgs, status: server, setServer }: {
  preset: SimplePreset;
  extraArgs: Record<string, string>;
  status: ServerStatus;
  setServer: (s: ServerStatus) => void;
}) {
  const [status, setStatus] = useState<BackendStatus | null>(null);
  const [installed, setInstalled] = useState<InstalledBackend[]>([]);
  const [serverRunning, setServerRunning] = useState(false);
  const [releases, setReleases] = useState<ReleaseInfo[]>([]);
  const [flavor, setFlavor] = useState("cpu");
  const [tag, setTag] = useState("");
  const [busy, setBusy] = useState("");
  const [progress, setProgress] = useState<string>("");
  const [error, setError] = useState("");
  const { t } = useTranslation();

  const refresh = async () => {
    try {
      const [s, list, srv] = await Promise.all([
        api.backendStatus(),
        api.listInstalledBackends(),
        api.serverStatus().catch(() => null),
      ]);
      setStatus(s);
      setInstalled(list);
      setServerRunning(srv?.running ?? false);
      if (srv) setServer(srv);
      setFlavor((f) => (s.suggested_flavor && !tag ? s.suggested_flavor : f));
    } catch (e) {
      setError(String(e));
    }
  };

  // The App topbar can stop/start the server while this tab is open: the
  // prop is fresher than the last manual refresh, so guards use both.
  const running = server.running || serverRunning;
  const syncServerState = async () => {
    try {
      const st = await api.serverStatus();
      setServer(st);
      setServerRunning(st.running);
    } catch {
      /* ignore */
    }
  };

  useEffect(() => {
    refresh();
    const un = listen<{ stage: string; downloaded: number; total: number | null }>(
      "backend-install-progress",
      (ev) => {
        const p = ev.payload;
        const total = p.total ? " / " + fmtBytes(p.total) : "";
        setProgress(
          p.stage === "download"
            ? t("backend.progressDownload", { done: fmtBytes(p.downloaded), total })
            : p.stage === "extract"
              ? t("backend.progressExtract")
              : t("backend.progressDone"),
        );
      },
    );
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadReleases = async () => {
    setBusy("releases");
    setError("");
    try {
      const r = await api.listReleases(5);
      setReleases(r);
      if (r.length > 0 && !tag) setTag(r[0].tag);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const install = async () => {
    const rel = releases.find((r) => r.tag === tag);
    const asset = rel?.assets.find((a) => a.flavor === flavor);
    if (!asset) {
      setError(t("backend.errNoAsset", { flavor, tag }));
      return;
    }
    if (running && !window.confirm(t("backend.confirmStopInstall"))) {
      return;
    }
    setBusy("install");
    setError("");
    try {
      const s = await api.installBackend(tag, flavor, asset.download_url);
      setStatus(s);
      setInstalled(await api.listInstalledBackends());
      await syncServerState();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const setDefault = async (tag: string, flavor: string) => {
    setError("");
    setBusy("switch");
    try {
      setInstalled(await api.setDefaultBackend(tag, flavor));
      setStatus(await api.backendStatus());
      // A running server still uses the old binary: restart it on the new
      // default with the current preset.
      if (running) {
        await api.stopServer();
        if (preset.model_path) {
          const st = await api.startServer(preset, extraArgs);
          setServer(st);
          setServerRunning(st.running);
        } else {
          const st = await api.serverStatus();
          setServer(st);
          setServerRunning(st.running);
          setError(t("backend.stoppedNoModel"));
        }
      }
    } catch (e) {
      setError(String(e));
      try {
        setServer(await api.serverStatus());
      } catch {
        /* ignore */
      }
    } finally {
      setBusy("");
    }
  };

  const remove = async (tag: string, flavor: string) => {
    if (running) {
      setError(t("backend.errRunningDelete"));
      return;
    }
    if (!window.confirm(t("backend.confirmDelete", { tag, flavor }))) {
      return;
    }
    setError("");
    try {
      setInstalled(await api.deleteBackend(tag, flavor));
      setStatus(await api.backendStatus());
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="page">
      <h2>{t("backend.title")}</h2>
      {error && <div className="alert error">{error}</div>}
      {installed.length > 0 && (
        <section className="card">
          <h3>{t("backend.installed")}</h3>
          {running && <p className="muted">{t("backend.runningNote")}</p>}
          <table className="tbl">
            <thead>
              <tr><th>{t("backend.colVersion")}</th><th>{t("backend.colFlavor")}</th><th>{t("backend.colSize")}</th><th></th></tr>
            </thead>
            <tbody>
              {installed.map((b) => (
                <tr key={`${b.tag}/${b.flavor}`} className={b.is_default ? "current" : ""}>
                  <td className="mono">{b.tag}{b.is_default && <span className="badge ok">{t("backend.badgeDefault")}</span>}</td>
                  <td><code>{b.flavor}</code></td>
                  <td>{fmtBytes(b.size_bytes)}</td>
                  <td className="cell-actions">
                    <div className="row">
                      {!b.is_default && <button onClick={() => setDefault(b.tag, b.flavor)} disabled={busy === "switch"}>{t("backend.setDefault")}</button>}
                      <button onClick={() => remove(b.tag, b.flavor)} disabled={running}>{t("common.delete")}</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      <section className="card">
        <h3>{t("backend.stateTitle")}</h3>
        {!status ? (
          <p>{t("common.loading")}</p>
        ) : status.installed ? (
          <table className="kv">
            <tbody>
              <tr><td>{t("backend.colVersion")}</td><td><code>{status.tag}</code></td></tr>
              <tr><td>{t("backend.colFlavor")}</td><td><code>{status.flavor}</code> <span className="muted">{t("backend.detected", { flavor: status.suggested_flavor })}</span></td></tr>
              <tr><td>{t("backend.binary")}</td><td className="mono">{status.server_exe}</td></tr>
              <tr><td>{t("backend.colSize")}</td><td>{fmtBytes(status.size_bytes)}</td></tr>
            </tbody>
          </table>
        ) : (
          <p>{t("backend.none")} <span className="muted">{t("backend.suggested", { flavor: status.suggested_flavor })}</span></p>
        )}
        <div className="row">
          <button onClick={refresh}>{t("common.refresh")}</button>
          <button onClick={loadReleases} disabled={busy === "releases"}>
            {busy === "releases" ? t("backend.checking") : t("backend.checkUpdates")}
          </button>
        </div>
      </section>

      {releases.length > 0 && (
        <section className="card">
          <h3>{t("backend.installTitle")}</h3>
          <div className="row">
            <label>{t("backend.release")}
              <select value={tag} onChange={(e) => setTag(e.target.value)}>
                {releases.map((r) => (
                  <option key={r.tag} value={r.tag}>{r.tag}</option>
                ))}
              </select>
            </label>
            <label>{t("backend.flavor")}
              <select value={flavor} onChange={(e) => setFlavor(e.target.value)}>
                {["cpu", "cuda", "vulkan", "hip"].map((f) => (
                  <option key={f} value={f}>{f}{status && f === status.suggested_flavor ? ` ${t("backend.detectedOpt")}` : ""}</option>
                ))}
              </select>
            </label>
            <button onClick={install} disabled={busy === "install"} className="primary">
              {busy === "install" ? t("backend.installing") : t("backend.install")}
            </button>
          </div>
          {progress && <p className="muted">{progress}</p>}
          <ul className="muted small">
            {releases.find((r) => r.tag === tag)?.assets.map((a) => (
              <li key={a.name}>{a.name} — {fmtBytes(a.size)} [{a.flavor}]</li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
