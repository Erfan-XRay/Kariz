import { useEffect, useState } from "react";
import { api, ApiError } from "./api";
import type { ScheduleRow } from "./api";
import { useApp } from "./store";
import { Dialog, Icon, Seg, useAgo } from "./ui";

type Kind = "tunnel" | "server";
type Mode = "every" | "daily";
type Unit = "m" | "h" | "d";

const UNIT_SECS: Record<Unit, number> = { m: 60, h: 3600, d: 86400 };
const MIN_SECS = 600;
const MAX_SECS = 30 * 86400;

const pad = (n: number) => String(n).padStart(2, "0");

/** The biggest unit that divides the interval evenly, for showing a saved one. */
function split(secs: number): { n: number; unit: Unit } {
  if (secs % 86400 === 0) return { n: secs / 86400, unit: "d" };
  if (secs % 3600 === 0) return { n: secs / 3600, unit: "h" };
  return { n: Math.max(1, Math.round(secs / 60)), unit: "m" };
}

/**
 * The time of day is typed in the browser's own time and kept in UTC, which is what the panel
 * counts in. `getTimezoneOffset` is UTC minus local, in minutes.
 */
const toUtcMinutes = (hhmm: string) => {
  const [h, m] = hhmm.split(":").map(Number);
  return (((h * 60 + m + new Date().getTimezoneOffset()) % 1440) + 1440) % 1440;
};
const toLocalTime = (utcMin: number) => {
  const local = (((utcMin - new Date().getTimezoneOffset()) % 1440) + 1440) % 1440;
  return `${pad(Math.floor(local / 60))}:${pad(local % 60)}`;
};

/** The setting and the state of one automatic restart. */
function AutoRestartForm({ kind, subject, offline }: { kind: Kind; subject: string; offline?: boolean }) {
  const { t, lang, num, toast, digitsOf } = useApp();
  const ago = useAgo();
  const [row, setRow] = useState<ScheduleRow | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [on, setOn] = useState(true);
  const [mode, setMode] = useState<Mode>("every");
  const [n, setN] = useState("6");
  const [unit, setUnit] = useState<Unit>("h");
  const [time, setTime] = useState("04:00");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // The list is read again every few seconds, for the result of the last run; the form is
  // filled in from it once.
  useEffect(() => {
    let alive = true;
    let first = true;
    const read = async () => {
      try {
        const { schedules } = await api.schedules();
        if (!alive) return;
        const mine = schedules.find((s) => s.kind === kind && s.subject === subject) ?? null;
        setRow(mine);
        if (first) {
          first = false;
          if (mine) {
            setOn(mine.enabled);
            setMode(mine.mode);
            if (mine.mode === "every") {
              const s = split(mine.every_secs);
              setN(String(s.n));
              setUnit(s.unit);
            } else {
              setTime(toLocalTime(mine.daily_min));
            }
          }
          setLoaded(true);
        }
      } catch {
        if (alive && first) setLoaded(true);
      }
    };
    void read();
    const id = setInterval(read, 5000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [kind, subject]);

  const secs = Math.round(Number(n) * UNIT_SECS[unit]);
  const badInterval = mode === "every" && (!Number.isFinite(secs) || secs < MIN_SECS || secs > MAX_SECS);

  const save = async () => {
    setError("");
    if (badInterval) {
      setError(t("ar.badInterval"));
      return;
    }
    setBusy(true);
    try {
      await api.setSchedule({
        kind,
        subject,
        mode,
        enabled: on,
        ...(mode === "every" ? { every_secs: secs } : { daily_min: toUtcMinutes(time) }),
      });
      toast(t(on ? "ar.saved" : "ar.savedOff"), "ok");
      const { schedules } = await api.schedules();
      setRow(schedules.find((s) => s.kind === kind && s.subject === subject) ?? null);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "bad_input" ? t("ar.badInterval") : t("ar.failedSave"));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await api.deleteSchedule(kind, subject);
      setRow(null);
      toast(t("ar.removed"), "ok");
    } catch {
      setError(t("ar.failedSave"));
    } finally {
      setBusy(false);
    }
  };

  const runNow = async () => {
    setBusy(true);
    try {
      await api.runSchedule(kind, subject);
      toast(t("ar.started"), "ok");
    } catch (e) {
      toast(e instanceof ApiError && e.code === "busy" ? t("ar.alreadyRunning") : t("ar.failedSave"), "err");
    } finally {
      setBusy(false);
    }
  };

  const when = (unix: number) =>
    digitsOf(
      new Date(unix * 1000).toLocaleString(lang === "fa" ? "fa-IR-u-nu-latn" : "en-GB", {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      }),
    );
  const result = (r: string) => {
    if (r === "") return "";
    const [code, rest] = [r.split(":")[0], r.includes(":") ? r.slice(r.indexOf(":") + 1) : ""];
    if (code === "ok") return rest ? t("ar.res.okN", { n: num(Number(rest)) }) : t("ar.res.ok");
    if (code === "failed") return t("ar.res.failed", { why: rest });
    return t(`ar.res.${code}`);
  };

  if (!loaded) return <p className="muted small">…</p>;

  return (
    <div className="ar">
      <label className="check">
        <input type="checkbox" checked={on} onChange={(e) => setOn(e.target.checked)} /> {t(kind === "tunnel" ? "ar.onTunnel" : "ar.onServer")}
      </label>
      <div className="field">
        <span className="help">{t(kind === "tunnel" ? "ar.helpTunnel" : "ar.helpServer")}</span>
      </div>
      <div className="field">
        <span className="label">{t("ar.mode")}</span>
        <Seg<Mode>
          value={mode}
          options={[
            ["every", t("ar.every")],
            ["daily", t("ar.daily")],
          ]}
          onChange={setMode}
        />
      </div>
      {mode === "every" ? (
        <div className="field">
          <label htmlFor={`ar-n-${kind}`}>{t("ar.interval")}</label>
          <div className="ar-row">
            <input id={`ar-n-${kind}`} className="text mono" dir="ltr" inputMode="numeric" value={n} onChange={(e) => setN(e.target.value.trim())} style={{ maxWidth: 110 }} />
            <Seg<Unit>
              value={unit}
              options={[
                ["m", t("ar.min")],
                ["h", t("ar.hour")],
                ["d", t("ar.day")],
              ]}
              onChange={setUnit}
            />
          </div>
          <span className="help">{t("ar.intervalHelp")}</span>
        </div>
      ) : (
        <div className="field">
          <label htmlFor={`ar-t-${kind}`}>{t("ar.time")}</label>
          <input id={`ar-t-${kind}`} className="text mono" type="time" dir="ltr" value={time} onChange={(e) => setTime(e.target.value)} style={{ maxWidth: 140 }} />
          <span className="help">{t("ar.timeHelp")}</span>
        </div>
      )}
      {error && <p className="err small" role="alert">{error}</p>}
      {offline && (
        <div className="field">
          <span className="help">{t("ar.offlineNote")}</span>
        </div>
      )}
      <div className="ar-actions">
        <button className="btn btn-primary btn-sm" type="button" disabled={busy} onClick={() => void save()}>
          {t("ar.save")}
        </button>
        {row && (
          <>
            <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={() => void runNow()}>
              <Icon name="restart" size={16} />
              {t("ar.runNow")}
            </button>
            <button className="btn btn-quiet btn-sm" type="button" disabled={busy} onClick={() => void remove()}>
              <Icon name="trash" size={16} />
              {t("ar.remove")}
            </button>
          </>
        )}
      </div>
      {row && (
        <dl className="ar-state">
          {row.enabled && row.next_run != null && (
            <div>
              <dt>{t("ar.next")}</dt>
              <dd>{when(row.next_run)}</dd>
            </div>
          )}
          {row.last_run > 0 && (
            <div>
              <dt>{t("ar.last")}</dt>
              <dd>{ago(Math.max(0, Date.now() / 1000 - row.last_run))}</dd>
            </div>
          )}
          {row.last_result && (
            <div>
              <dt>{t("ar.result")}</dt>
              <dd className={row.last_result.startsWith("failed") ? "warn-text" : ""}>{result(row.last_result)}</dd>
            </div>
          )}
        </dl>
      )}
    </div>
  );
}

/** The automatic restart of a tunnel, as a block on its page. */
export function AutoRestartBlock({ tunnel }: { tunnel: string }) {
  return <AutoRestartForm kind="tunnel" subject={tunnel} />;
}

/** The automatic restart of a server's running tunnels, in a dialog. */
export function AutoRestartDialog({ server, name, offline, onClose }: { server: string; name: string; offline: boolean; onClose: () => void }) {
  const { t } = useApp();
  return (
    <Dialog
      title={t("ar.serverTitle", { name })}
      onClose={onClose}
      footer={
        <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
          {t("close")}
        </button>
      }
    >
      <AutoRestartForm kind="server" subject={server} offline={offline} />
    </Dialog>
  );
}
