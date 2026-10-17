import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { api, fmtBytes, DEFAULT_REPO, type LocalModel, type RemoteFile } from "../lib/api";

export default function ModelsPage({ selected, hasRouter, onSelect }: { selected: string; hasRouter: boolean; onSelect: (path: string) => void }) {
  const [local, setLocal] = useState<LocalModel[]>([]);
  const [repo, setRepo] = useState(DEFAULT_REPO);
  const [token, setToken] = useState("");
  const [remote, setRemote] = useState<RemoteFile[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState("");
  const [progress, setProgress] = useState("");
  const [error, setError] = useState("");
  const { t } = useTranslation();

  const refreshLocal = async () => {
    try {
      setLocal(await api.listLocalModels());
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    refreshLocal();
    const un = listen<{ file: string; downloaded: number; total: number | null; stage: string }>(
      "model-download-progress",
      (ev) => {
        const p = ev.payload;
        setProgress(
          p.stage === "download"
            ? `${p.file} : ${fmtBytes(p.downloaded)}${p.total ? " / " + fmtBytes(p.total) : ""}`
            : t("models.progressDone", { file: p.file }),
        );
        if (p.stage === "done") refreshLocal();
      },
    );
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const listRemote = async () => {
    setBusy("list");
    setError("");
    try {
      const files = await api.listRemoteFiles(repo, token || undefined);
      setRemote(files);
      const pre = files.filter((f) => f.recommended).map((f) => f.name);
      setChecked(new Set(pre.length > 0 ? pre.slice(0, 1) : []));
      if (files.length === 0) setError(t("models.errNoneFound"));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const downloadChecked = async () => {
    setBusy("dl");
    setError("");
    try {
      for (const name of checked) {
        await api.downloadModel(repo, name, token || undefined);
      }
      await refreshLocal();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const remove = async (path: string) => {
    if (!confirm(t("models.confirmDelete", { path }))) return;
    try {
      await api.deleteModel(path);
      await refreshLocal();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggle = (name: string) => {
    const next = new Set(checked);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    setChecked(next);
  };

  return (
    <div className="page">
      <h2>{t("models.title")}</h2>
      {error && <div className="alert error">{error}</div>}

      <section className="card">
        <h3>{t("models.local")}</h3>
        {local.length === 0 ? (
          <p className="muted">{t("models.noneDownloaded")}</p>
        ) : (
          <table className="tbl">
            <thead><tr><th>{t("models.colFile")}</th><th>{t("models.colQuant")}</th><th>{t("models.colSize")}</th><th></th></tr></thead>
            <tbody>
              {local.map((m) => (
                <tr key={m.file} className={m.file === selected ? "current" : ""}>
                  <td className="mono">{m.file.split(/[/\\]/).pop()}{m.file === selected && !hasRouter && <span className="badge ok">{t("models.badgeUsed")}</span>}</td>
                  <td><code>{m.quant}</code></td>
                  <td>{fmtBytes(m.size_bytes)}</td>
                  <td className="cell-actions">
                    <div className="row">
                      {/* In router mode the client picks the model per
                          request: there is nothing to "use" from here,
                          exposure is managed in the Server tab. */}
                      {!hasRouter && <button onClick={() => onSelect(m.file)}>{t("models.use")}</button>}
                      <button onClick={() => remove(m.file)}>{t("common.delete")}</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div className="row"><button onClick={refreshLocal}>{t("common.refresh")}</button></div>
      </section>

      <section className="card">
        <h3>{t("models.downloadTitle")}</h3>
        <div className="row">
          <label className="grow">{t("models.repo")}
            <input value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="owner/repo" />
          </label>
          <label>{t("models.token")}
            <input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder="hf_…" />
          </label>
          <button onClick={listRemote} disabled={busy === "list"}>
            {busy === "list" ? t("models.listing") : t("models.list")}
          </button>
        </div>
        {remote.length > 0 && (
          <>
            <ul className="checklist">
              {remote.map((f) => (
                <li key={f.name}>
                  <label>
                    <input type="checkbox" checked={checked.has(f.name)} onChange={() => toggle(f.name)} />
                    <span className="mono">{f.name}</span>
                    {f.recommended && <span className="badge ok">{t("models.badgeRecommended")}</span>}
                    {f.universal && <span className="badge">{t("models.badgeUniversal")}</span>}
                    {/q2_0\.gguf$/i.test(f.name) && !/g64/i.test(f.name) && (
                      <span className="badge warn">{t("models.badgeLegacy")}</span>
                    )}
                  </label>
                </li>
              ))}
            </ul>
            <div className="row">
              <button onClick={downloadChecked} disabled={busy === "dl" || checked.size === 0} className="primary">
                {busy === "dl" ? t("models.downloading") : t("models.downloadBtn", { n: checked.size })}
              </button>
            </div>
          </>
        )}
        {progress && <p className="muted">{progress}</p>}
      </section>
    </div>
  );
}
