import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, serverHost, type HelpSchema, type HelpOption, type RouterConfig, type ServerStatus, type SimplePreset } from "../lib/api";

interface Props {
  preset: SimplePreset;
  setPreset: (p: SimplePreset) => void;
  extraArgs: Record<string, string>;
  setExtraArgs: (e: Record<string, string>) => void;
  status: ServerStatus;
  setStatus: (s: ServerStatus) => void;
  router: RouterConfig;
}

function kindOf(o: HelpOption): string {
  if (typeof o.kind === "string") return o.kind;
  const k = Object.keys(o.kind)[0] ?? "text";
  return k;
}

function enumValues(o: HelpOption): string[] {
  if (typeof o.kind === "object" && o.kind && "enum" in o.kind) {
    return (o.kind as { enum: string[] }).enum;
  }
  return [];
}

export default function ServerPage({ preset, setPreset, extraArgs, setExtraArgs, status, setStatus, router }: Props) {
  const [mode, setMode] = useState<"simple" | "expert">("simple");
  const hasRouter = router.models.some((m) => m.routed);
  const [schema, setSchema] = useState<HelpSchema | null>(null);
  const [section, setSection] = useState("");
  const [search, setSearch] = useState("");
  const [cmdline, setCmdline] = useState<string[]>([]);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const { t } = useTranslation();

  const loadSchema = async () => {
    setBusy("help");
    setError("");
    try {
      const s = await api.parseHelp();
      setSchema(s);
      if (!section && s.sections.length > 0) setSection(s.sections[0]);
    } catch (e) {
      setError(t("server.errHelp", { error: String(e) }));
    } finally {
      setBusy("");
    }
  };

  const refreshCmd = async () => {
    try {
      setCmdline(await api.buildCmdline(preset, extraArgs));
    } catch {
      /* ignore */
    }
  };

  useEffect(() => {
    refreshCmd();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preset, extraArgs]);

  const set = (k: keyof SimplePreset, v: string | number) =>
    setPreset({ ...preset, [k]: v });

  const setExtra = (flag: string, value: string) => {
    const next = { ...extraArgs };
    if (value === "__UNSET__") delete next[flag];
    else next[flag] = value;
    setExtraArgs(next);
  };

  const start = async () => {
    setBusy("start");
    setError("");
    try {
      setStatus(await api.startServer(preset, extraArgs, router));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const stop = async () => {
    setBusy("stop");
    try {
      setStatus(await api.stopServer());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const filtered = useMemo(() => {
    if (!schema) return [];
    const q = search.toLowerCase();
    return schema.options.filter(
      (o) =>
        (!section || o.section === section) &&
        (!q || o.flag.toLowerCase().includes(q) || o.description.toLowerCase().includes(q) || o.aliases.some((a) => a.toLowerCase().includes(q))),
    );
  }, [schema, section, search]);

  const renderControl = (o: HelpOption) => {
    const kind = kindOf(o);
    const val = extraArgs[o.flag];
    const isSet = val !== undefined;
    if (kind === "bool") {
      return (
        <input
          type="checkbox"
          checked={isSet}
          onChange={(e) => setExtra(o.flag, e.target.checked ? "" : "__UNSET__")}
        />
      );
    }
    if (kind === "enum") {
      const vals = enumValues(o);
      return (
        <span className="row">
          <select value={val ?? ""} onChange={(e) => setExtra(o.flag, e.target.value || "__UNSET__")}>
            <option value="">{t("server.defaultOpt")}</option>
            {vals.map((v) => (
              <option key={v} value={v}>{v}</option>
            ))}
          </select>
        </span>
      );
    }
    if (kind === "int" || kind === "float") {
      return (
        <input
          type="number"
          step={kind === "float" ? "0.01" : "1"}
          placeholder={o.default ?? t("server.defaultValue")}
          value={val ?? ""}
          onChange={(e) => setExtra(o.flag, e.target.value ? e.target.value : "__UNSET__")}
        />
      );
    }
    return (
      <input
        type="text"
        placeholder={o.default ?? "defaut"}
        value={val ?? ""}
        onChange={(e) => setExtra(o.flag, e.target.value ? e.target.value : "__UNSET__")}
      />
    );
  };

  return (
    <div className="page">
      <h2>{t("server.title")}</h2>
      {error && <div className="alert error">{error}</div>}

      <div className="row">
        <div className="tabs small">
          <button className={mode === "simple" ? "active" : ""} onClick={() => setMode("simple")}>{t("server.simple")}</button>
          <button className={mode === "expert" ? "active" : ""} onClick={() => { setMode("expert"); if (!schema) loadSchema(); }}>{t("server.expert")}</button>
        </div>
        <span className={`dot ${status.running ? "on" : "off"}`} />
        <span className="muted">{status.running ? t("server.running", { host: serverHost(preset, extraArgs), port: status.port, pid: status.pid }) : t("server.stopped")}</span>
        <span className="spacer" />
        {!status.running ? (
          <button
            onClick={start}
            disabled={busy === "start" || (!hasRouter && !preset.model_path)}
            className="primary"
            title={!hasRouter && !preset.model_path ? t("topbar.chooseModelFirst") : ""}
          >
            {busy === "start" ? t("server.starting") : t("server.start")}
          </button>
        ) : (
          <button onClick={stop} disabled={busy === "stop"}>{t("server.stop")}</button>
        )}
      </div>

      {hasRouter && mode === "simple" && (
        <div className="alert info">{t("server.routerNote")}</div>
      )}

      {mode === "simple" && (
        <section className="card">
          <h3>{t("server.simpleTitle")}</h3>
          <div className="grid2">
            <label>{t("server.model")}<input value={preset.model_path} onChange={(e) => set("model_path", e.target.value)} className="mono" placeholder={t("server.modelPh")} /></label>
            <label>{t("server.vision")}<input value={preset.mmproj_path ?? ""} onChange={(e) => set("mmproj_path", e.target.value)} className="mono" placeholder="…-mmproj.gguf" /></label>
            <label>{t("server.ctx")}<input type="number" value={preset.ctx} onChange={(e) => set("ctx", Number(e.target.value))} /></label>
            <label>{t("server.layers")}<input type="number" value={preset.ngl} onChange={(e) => set("ngl", Number(e.target.value))} /></label>
            <label>{t("server.threads")}<input type="number" value={preset.threads} onChange={(e) => set("threads", Number(e.target.value))} /></label>
            <label>{t("server.port")}<input type="number" value={preset.port} onChange={(e) => set("port", Number(e.target.value))} /></label>
            <label>{t("server.host")}<input value={preset.host} onChange={(e) => set("host", e.target.value)} /></label>
            <label>{t("server.effort")}
              <select value={preset.reasoning_effort} onChange={(e) => set("reasoning_effort", e.target.value)}>
                <option value="xhigh">{t("server.effortXhigh")}</option>
                <option value="medium">{t("server.effortMedium")}</option>
              </select>
            </label>
          </div>
          <p className="muted small">{t("server.samplingNote")}</p>
        </section>
      )}

      {mode === "expert" && (
        <section className="card">
          <h3>{t("server.expertTitle", { cmd: "llama-server --help" })}</h3>
          <div className="row">
            <button onClick={loadSchema} disabled={busy === "help"}>{busy === "help" ? t("server.parsing") : t("server.reparse")}</button>
            <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder={t("server.searchPh")} />
            {schema?.raw_version && <span className="muted small">{schema.raw_version}</span>}
            {schema && <span className="muted small">{t("server.optionsCount", { n: schema.options.length })}</span>}
          </div>
          {schema && (
            <>
              <div className="tabs small wrap">
                {schema.sections.map((s) => (
                  <button key={s} className={section === s ? "active" : ""} onClick={() => setSection(s)}>{s}</button>
                ))}
              </div>
              <table className="tbl">
                <thead><tr><th>{t("server.colFlag")}</th><th>{t("server.colValue")}</th><th>{t("server.colDesc")}</th></tr></thead>
                <tbody>
                  {filtered.slice(0, 300).map((o) => (
                    <tr key={o.flag} className={extraArgs[o.flag] !== undefined ? "modified" : ""}>
                      <td className="mono">{o.flag}{o.value_name ? ` ${o.value_name}` : ""}{o.aliases.length > 0 && <span className="muted"> ({o.aliases.join(", ")})</span>}{extraArgs[o.flag] !== undefined && <span className="badge">{t("server.badgeModified")}</span>}</td>
                      <td>{renderControl(o)}</td>
                      <td className="muted small">{o.description}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {filtered.length > 300 && <p className="muted small">{t("server.limited")}</p>}
              <details className="muted small">
                <summary>{t("server.rawHelp", { n: schema.raw.split("\n").length })}</summary>
                <pre className="log">{schema.raw.slice(0, 20000)}</pre>
              </details>
            </>
          )}
        </section>
      )}

      <section className="card">
        <h3>{t("server.cmdline")}</h3>
        <pre className="cmd">llama-server {cmdline.map((a) => (a.includes(" ") ? `"${a}"` : a)).join(" ")}</pre>
      </section>
    </div>
  );
}
