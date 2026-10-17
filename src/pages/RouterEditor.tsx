import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  api, suggestAlias,
  type HelpOption, type HelpSchema, type IniError, type LocalModel,
  type RouterConfig, type RouterModel,
} from "../lib/api";

interface Props {
  router: RouterConfig;
  setRouter: (c: RouterConfig) => void;
  /// Persist through the backend (validates against `--help`, writes the
  /// INI). Returns the list of rejected keys.
  saveRouter: (c: RouterConfig) => Promise<IniError[]>;
}

/// Keys the router owns: in the preset they are overwritten by the server
/// anyway, so they are not offered here.
const ROUTER_KEYS = [
  "host", "port", "api-key", "api-key-file", "alias", "hf-repo", "hf-file",
  "models-dir", "models-preset", "models-max", "models-autoload",
  "no-models-autoload", "sleep-idle-seconds",
];
const PRESET_ONLY_KEYS = ["load-on-startup", "stop-timeout", "dedup-cache-models"];

function iniKey(flag: string): string {
  return flag.replace(/^-+/, "");
}

function kindOf(o: HelpOption): string {
  if (typeof o.kind === "string") return o.kind;
  return Object.keys(o.kind)[0] ?? "text";
}

export default function RouterEditor({ router, setRouter, saveRouter }: Props) {
  const { t } = useTranslation();
  const [schema, setSchema] = useState<HelpSchema | null>(null);
  const [local, setLocal] = useState<LocalModel[]>([]);
  const [pick, setPick] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [errs, setErrs] = useState<IniError[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.parseHelp().then(setSchema).catch(() => setSchema(null));
    api.listLocalModels().then(setLocal).catch(() => setLocal([]));
  }, []);

  /// Options usable as overrides: known to the `--help` schema and not
  /// router-owned. The schema's sections give the natural buckets.
  const options = useMemo<HelpOption[]>(() => {
    if (!schema) return [];
    return schema.options.filter((o) => {
      const k = iniKey(o.flag);
      return (
        !ROUTER_KEYS.includes(k) &&
        !PRESET_ONLY_KEYS.includes(k) &&
        k !== "model" && // written from model_path
        k !== "help"
      );
    });
  }, [schema]);

  const apply = async (next: RouterConfig) => {
    setRouter(next);
    setBusy(true);
    try {
      setErrs(await saveRouter(next));
    } catch (e) {
      console.error(e);
      setErrs([]);
    } finally {
      setBusy(false);
    }
  };

  const patchModel = (alias: string, patch: Partial<RouterModel>) =>
    apply({ ...router, models: router.models.map((m) => (m.alias === alias ? { ...m, ...patch } : m)) });

  const addModel = (file: string) => {
    if (!file) return;
    let alias = suggestAlias(file);
    // Unique alias: it is the INI section name and the API model name.
    let n = 2;
    while (router.models.some((m) => m.alias === alias)) alias = `${suggestAlias(file)}-${n++}`;
    apply({
      ...router,
      models: [...router.models, { alias, model_path: file, overrides: [], load_on_startup: false }],
    });
    setEditing(alias);
    setPick("");
  };

  const setPair = (pairs: [string, string][], key: string, value: string): [string, string][] => {
    const k = iniKey(key);
    const rest: [string, string][] = pairs.filter(([pk]) => pk !== k);
    return value === "__UNSET__" ? rest : [...rest, [k, value]];
  };

  const overrideRows = (pairs: [string, string][], set: (p: [string, string][]) => void) => (
    <table className="tbl">
      <thead>
        <tr><th>{t("router.colParam")}</th><th>{t("router.colValue")}</th><th /></tr>
      </thead>
      <tbody>
        {pairs.map(([k, v]) => {
          const opt = options.find((o) => iniKey(o.flag) === k);
          const kind = opt ? kindOf(opt) : "text";
          return (
            <tr key={k}>
              <td className="mono">{k}</td>
              <td>
                {kind === "bool" ? (
                  <label className="row">
                    <input
                      type="checkbox"
                      checked={v === "true" || v === ""}
                      onChange={(e) => set(setPair(pairs, k, e.target.checked ? "true" : "__UNSET__"))}
                    />
                  </label>
                ) : (
                  <input
                    type={kind === "int" || kind === "float" ? "number" : "text"}
                    placeholder={opt?.default ?? ""}
                    value={v}
                    onChange={(e) => set(setPair(pairs, k, e.target.value))}
                  />
                )}
              </td>
              <td>
                <button onClick={() => set(setPair(pairs, k, "__UNSET__"))}>{t("router.remove")}</button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );

  const picker = (pairs: [string, string][], set: (p: [string, string][]) => void) => (
    <select
      value=""
      onChange={(e) => {
        const k = e.target.value;
        if (!k) return;
        const opt = options.find((o) => iniKey(o.flag) === k);
        const init: [string, string] = [
          iniKey(k),
          kindOf(opt ?? ({} as HelpOption)) === "bool" ? "true" : "",
        ];
        set([...pairs, init]);
      }}
    >
      <option value="">{t("router.addParam")}</option>
      {options
        .filter((o) => !pairs.some(([k]) => k === iniKey(o.flag)))
        .map((o) => (
          <option key={o.flag} value={o.flag}>{o.flag}</option>
        ))}
    </select>
  );

  return (
    <section className="card">
      <div className="row">
        <h3>{t("router.title")}</h3>
        <span className="spacer" />
        <span className="muted small">{t("router.mode", { n: router.models.length })}</span>
      </div>
      <p className="muted small">{t("router.intro")}</p>

      {errs.length > 0 && (
        <div className="alert error">
          <ul>
            {errs.slice(0, 8).map((e, i) => (
              <li key={i}>
                {e.scope === "*" ? t("router.scopeGlobal") : t("router.scopeModel", { alias: e.scope })}
                {" — "}
                <span className="mono">{e.key}</span>
                {" "}
                {t(`router.err.${e.reason}`)}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* --- global [*] section -------------------------------------- */}
      <details open={!router.models.length}>
        <summary>{t("router.globals")}</summary>
        <p className="muted small">{t("router.globalsHelp")}</p>
        {overrideRows(router.global, (p) => apply({ ...router, global: p }))}
        <div className="row">{picker(router.global, (p) => apply({ ...router, global: p }))}</div>
      </details>

      {/* --- one section per model ------------------------------------ */}
      <div className="row">
        <h3>{t("router.models", { n: router.models.length })}</h3>
        <span className="spacer" />
        <select value={pick} onChange={(e) => addModel(e.target.value)} disabled={busy || local.length === 0}>
          <option value="">{t("router.addModel")}</option>
          {local.map((m) => (
            <option key={m.file} value={m.file}>{suggestAlias(m.file)} — {m.file.split(/[\\/]/).pop()}</option>
          ))}
        </select>
      </div>

      {router.models.length === 0 && <p className="muted small">{t("router.noModel")}</p>}
      {local.length === 0 && <p className="muted small">{t("router.noLocal")}</p>}

      {router.models.map((m) => (
        <div key={m.alias} className="router-model">
          <div className="row">
            <strong className="mono">{m.alias}</strong>
            <span className="muted small mono ellipsis" title={m.model_path}>
              {m.model_path.split(/[\\/]/).pop()}
            </span>
            <span className="spacer" />
            <label className="row small">
              <input
                type="checkbox"
                checked={m.load_on_startup}
                onChange={(e) => patchModel(m.alias, { load_on_startup: e.target.checked })}
              />
              {t("router.loadOnStartup")}
            </label>
            <button onClick={() => apply({ ...router, models: router.models.filter((x) => x.alias !== m.alias) })}>
              {t("common.delete")}
            </button>
          </div>
          {editing === m.alias ? (
            <>
              {overrideRows(m.overrides, (p) => patchModel(m.alias, { overrides: p }))}
              <div className="row">
                {picker(m.overrides, (p) => patchModel(m.alias, { overrides: p }))}
                <button onClick={() => setEditing(null)}>{t("router.done")}</button>
              </div>
            </>
          ) : (
            <div className="row">
              <button onClick={() => setEditing(m.alias)}>{t("router.params", { n: m.overrides.length })}</button>
            </div>
          )}
        </div>
      ))}
      {schema && <p className="muted small">{t("router.optionsCount", { n: options.length })}</p>}
    </section>
  );
}
