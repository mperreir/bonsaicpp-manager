import { useTranslation } from "react-i18next";
import { fmtPct, fmtPctRatio, fmtSecs, fmtSpec, fmtTps, numLocale, type ServerStatus } from "../lib/api";
import type { StatsBundle } from "../hooks/useStats";

interface Props {
  status: ServerStatus;
  stats: StatsBundle;
}

export default function StatsPage({ status, stats }: Props) {
  const { activity, error } = stats;

  const history = activity?.history ?? [];
  const models = activity?.models ?? [];
  const routerOn = activity?.router ?? false;
  const ctxMax = activity?.ctx_max ?? null;
  const ctxUsed = activity?.ctx_used ?? 0;
  const ctxPct = ctxMax && ctxMax > 0 ? Math.min(100, (ctxUsed / ctxMax) * 100) : null;
  const { t } = useTranslation();
  const loc = numLocale();

  return (
    <div className="page fill">
      <h2>{t("stats.title")}</h2>
      {error && <div className="alert error">{error}</div>}
      {!status.running ? (
        <p className="muted">{t("stats.stoppedNote")}</p>
      ) : (
        <>
          {routerOn && models.length > 0 && (
            <div className="row">
              <span className="muted small">{t("stats.model")}</span>
              <span className="chip-row">
                <span className={`chip ${activity?.selected_model ? "on" : ""}`}>{t("stats.modelAuto")}</span>
                {models.map((m) => (
                  <span
                    key={m.alias}
                    className={`chip m-${m.status} ${activity?.selected_model === m.alias ? "on" : ""}`}
                    title={t(`router.status.${m.status}`, { alias: m.alias })}
                  >
                    {m.alias}
                  </span>
                ))}
              </span>
              <span className="spacer" />
              {activity?.selected_model && <span className="muted small">{t("stats.showing", { alias: activity.selected_model })}</span>}
            </div>
          )}
          <p className="muted small seclabel">{t("stats.live")}</p>
          <div className="statgrid">
            <div className="stat">
              <span className="label">{t("stats.prefill")}</span>
              <span className="value">{fmtTps(activity?.prefill_tps)}</span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.decode")}</span>
              <span className="value">{fmtTps(activity?.decode_tps)}</span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.promptCache")}</span>
              <span className="value">{fmtPct(activity?.cache_live)}</span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.activeSlots")}</span>
              <span className="value">{activity?.active_slots.length ?? "—"}</span>
              <span className="muted small">
                {activity && activity.active_slots.length > 0
                  ? activity.active_slots.map((s) =>
                    `slot ${s.id} ${s.phase === "prefill" ? t("stats.slotPrefill") : t("stats.slotDecode")}` +
                    (s.phase === "prefill" && s.prompt_total > 0 ? ` ${s.prompt_done}/${s.prompt_total}` : "") +
                    (s.phase === "decode" ? ` ${s.decoded} tok` : "") +
                    (s.live_tps != null ? ` ${s.live_tps.toFixed(1)} t/s` : "")
                  ).join(" • ")
                  : t("stats.queued", { n: activity?.requests_deferred ?? "—" })}
              </span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.context")}</span>
              <span className="value">
                {activity ? ctxUsed.toLocaleString(loc) : "—"}
                <span className="muted small"> / {ctxMax != null ? ctxMax.toLocaleString(loc) : "—"}</span>
              </span>
              <span className="muted small">
                {ctxPct != null ? t("stats.ctxUsed", { pct: ctxPct.toFixed(1) }) : t("stats.ctxWaiting")}
              </span>
            </div>
          </div>

          <p className="muted small seclabel">{t("stats.avg")}</p>
          <div className="statgrid">
            <div className="stat">
              <span className="label">{t("stats.avgPrefill")}</span>
              <span className="value">{fmtTps(activity?.avg_prefill_tps)}</span>
              <span className="muted small">
                {(activity?.avg_prefill_polls ?? 0) > 0 ? t("stats.polls", { n: activity?.avg_prefill_polls }) : "—"}
              </span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.avgDecode")}</span>
              <span className="value">{fmtTps(activity?.avg_decode_tps)}</span>
              <span className="muted small">
                {(activity?.avg_decode_polls ?? 0) > 0 ? t("stats.polls", { n: activity?.avg_decode_polls }) : "—"}
              </span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.sessionCache")}</span>
              <span className="value">{fmtPct(activity?.cache_session)}</span>
              <span className="muted small">
                {activity ? t("stats.cachePair", { a: activity.cache_session_cached.toLocaleString(loc), d: activity.cache_session_total.toLocaleString(loc) }) : "—"}
              </span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.spec")}</span>
              <span className="value">{fmtPct(activity?.spec_accept)}</span>
              <span className="muted small">
                {activity ? t("stats.drafts", { a: activity.spec_accepted.toLocaleString(loc), d: activity.spec_drafted.toLocaleString(loc) }) : "—"}
              </span>
            </div>
            <div className="stat">
              <span className="label">{t("stats.ttft")}</span>
              <span className="value">{fmtSecs(activity?.ttft_last)}</span>
              <span className="muted small">
                {(activity?.ttft_n ?? 0) > 0 ? t("stats.ttftAvg", { avg: fmtSecs(activity?.ttft_avg), n: activity?.ttft_n }) : t("stats.noReq")}
              </span>
            </div>
          </div>

          <section className="card fill">
            <h3>{t("stats.history", { n: history.length })}</h3>
            {history.length === 0 ? (
              <p className="muted">{t("stats.historyEmpty")}</p>
            ) : (
              <div className="tblscroll">
              <table className="tbl">
                <thead><tr>{routerOn && <th>{t("stats.colModel")}</th>}<th>{t("stats.colTime")}</th><th>{t("stats.colPrompt")}</th><th>{t("stats.colPrefill")}</th><th>{t("stats.colCache")}</th><th>{t("stats.colTtft")}</th><th>{t("stats.colGenerated")}</th><th>{t("stats.colDecode")}</th><th>{t("stats.colSpec")}</th></tr></thead>
                <tbody>
                  {[...history].reverse().slice(0, 20).map((s, i) => (
                    <tr key={`${s.at}-${i}`}>
                      {routerOn && <td className="mono small">{s.model ?? "—"}</td>}
                      <td className="muted small">{new Date(s.at).toLocaleTimeString(loc)}</td>
                      <td>{s.prompt_n ?? "—"}</td>
                      <td>{fmtTps(s.prompt_tps)}</td>
                      <td>{fmtPctRatio(s.cache_n, s.cache_total_n)}</td>
                      <td>{fmtSecs(s.prompt_ms != null ? s.prompt_ms / 1000 : null)}</td>
                      <td>{s.predicted_n ?? "—"}</td>
                      <td>{fmtTps(s.predicted_tps)}</td>
                      <td>{fmtSpec(s.spec_drafted, s.spec_accepted)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              </div>
            )}
          </section>
        </>
      )}
    </div>
  );
}
