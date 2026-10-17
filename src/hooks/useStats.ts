import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, type Activity, type ServerStatus } from "../lib/api";

/// Stats polling at App level: survives tab switches and hiding
/// to the tray (the Stats tab itself unmounts). Single loop.
///
/// The backend returns everything (live + averages + history) on each poll:
/// this hook only broadcasts, no accumulation on the React side.
export function useStats(status: ServerStatus) {
  const [activity, setActivity] = useState<Activity | null>(null);
  const [error, setError] = useState("");
  const [busyTest, setBusyTest] = useState(false);
  const [tooltipOn, setTooltipOnState] = useState(() => {
    try {
      return localStorage.getItem("bonsai.tooltip") !== "0";
    } catch {
      return true;
    }
  });
  const [pollMs, setPollMsState] = useState(() =>
    Number(localStorage.getItem("bonsai.pollMs") ?? 1000));
  const [pollDuration, setPollDuration] = useState<number | null>(null);
  const lastTip = useRef("");
  const pollRunning = useRef(false);
  const activityRef = useRef<Activity | null>(null);
  const tooltipOnRef = useRef(tooltipOn);
  tooltipOnRef.current = tooltipOn;
  const { t } = useTranslation();
  const tipRef = useRef(t);
  tipRef.current = t;

  const setTooltipOn = (v: boolean) => {
    setTooltipOnState(v);
    try {
      localStorage.setItem("bonsai.tooltip", v ? "1" : "0");
    } catch {
      /* ignore */
    }
  };

  const setPollMs = (ms: number) => {
    setPollMsState(ms);
    localStorage.setItem("bonsai.pollMs", String(ms));
  };

  const poll = async () => {
    if (!status.running || !status.port) return;
    if (pollRunning.current) return; // no overlap if the previous poll lags
    pollRunning.current = true;
    const t0 = performance.now();
    try {
      const a = await api.pollActivity(status.port);
      setActivity(a);
      activityRef.current = a;
      setError("");
      // Tooltip: decode speed when decoding, else just "ready".
      // (Prefill-only phases are brief; the tooltip stays quiet.)
      if (tooltipOnRef.current) {
        const tr = tipRef.current;
        // Router mode: name the model the speeds belong to.
        const who = a.router && a.selected_model ? `${a.selected_model} • ` : "";
        const tip = a.decode_tps != null
          ? `${who}Bonsai • ${
            [
              a.prefill_tps != null ? tr("tray.prefill", { tps: a.prefill_tps.toFixed(0) }) : null,
              tr("tray.decode", { tps: a.decode_tps.toFixed(1) }),
            ].filter(Boolean).join(" • ")
          }`
          : tr("tray.ready");
        if (tip !== lastTip.current) {
          lastTip.current = tip;
          await api.setTrayTooltip(tip);
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setPollDuration(Math.round(performance.now() - t0));
      pollRunning.current = false;
    }
  };

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      await poll();
      if (!alive) return;
      // Adaptive: 0.5 s during generations, chosen frequency when idle.
      const active = (activityRef.current?.active_slots.length ?? 0) > 0;
      timer = setTimeout(tick, active ? 500 : pollMs);
    };
    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status.running, status.port, pollMs]);

  const sendTest = async () => {
    if (!status.port) return;
    setBusyTest(true);
    setError("");
    try {
      // The test request is recorded in the backend history;
      // repoll right away to show it without waiting for the next tick.
      await api.sendTestPrompt(status.port);
      await poll();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyTest(false);
    }
  };

  return {
    activity, error, busyTest,
    tooltipOn, setTooltipOn, pollMs, setPollMs, pollDuration, sendTest,
  };
}

export type StatsBundle = ReturnType<typeof useStats>;
