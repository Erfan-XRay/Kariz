import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import type { Quota, QuotaState, UsageOf, UsageRow, UsageSummary, UsageTotals } from "./api";
import * as cal from "./calendar";
import type { UsageSettings } from "./calendar";
import { useApp } from "./store";
import { Dialog, Icon, Seg, Sep } from "./ui";

export const ZERO: UsageRow = { rx: 0, tx: 0, data: 0, proto: 0, resend: 0, headers: 0, up: 0, down: 0, est: 0 };

function add(a: UsageRow, b: UsageRow): UsageRow {
  return {
    rx: a.rx + b.rx,
    tx: a.tx + b.tx,
    data: a.data + b.data,
    proto: a.proto + b.proto,
    resend: a.resend + b.resend,
    headers: a.headers + b.headers,
    up: a.up + b.up,
    down: a.down + b.down,
    est: a.est + b.est,
  };
}

const total = (r: UsageRow) => r.rx + r.tx;

/** Bytes as a number and a unit (binary steps, like every other volume in the panel). */
export function volume(n: number): { value: number; unit: string; decimals: number } {
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return { value: v, unit: units[i], decimals: i === 0 || v >= 100 ? 0 : v >= 10 ? 1 : 2 };
}

/** A share inside a sentence, with the percent sign its language writes. */
function usePercent() {
  const { num, lang } = useApp();
  return (v: number) => `${num(v, v < 10 ? 1 : 0)}${lang === "fa" ? "٪" : "%"}`;
}

/** A volume inside a sentence, kept in one piece (number before unit) in right-to-left text. */
function useVolumeText() {
  const { num } = useApp();
  return (n: number) => {
    const v = volume(n);
    return `⁦${num(v.value, v.decimals)} ${v.unit}⁩`;
  };
}

// ---- one shared summary for every part of a page, read every 30 s ----

interface Marks {
  today: number;
  week: number;
  month: number;
}

interface Shared {
  settings: UsageSettings | null;
  summary: UsageSummary | null;
  marks: Marks | null;
}

let shared: Shared = { settings: null, summary: null, marks: null };
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function marksOf(s: UsageSettings): Marks {
  const day = cal.today(s);
  return {
    today: cal.dayStart(s, day),
    week: cal.dayStart(s, cal.weekStart(s.calendar, day)),
    month: cal.dayStart(s, cal.monthStart(s.calendar, day)),
  };
}

async function refresh() {
  try {
    const settings = await api.usageSettings();
    const marks = marksOf(settings);
    const summary = await api.usageSummary([marks.today, marks.week, marks.month]);
    shared = { settings, summary, marks };
    listeners.forEach((f) => f());
  } catch {
    // shown again on the next round
  }
}

/** The usage of every server and tunnel, and how it is shown (calendar, time zone). */
export function useUsage(): Shared & { reload: () => void } {
  const [, redraw] = useState(0);
  useEffect(() => {
    const f = () => redraw((x) => x + 1);
    listeners.add(f);
    if (listeners.size === 1) {
      void refresh();
      timer = setInterval(() => void refresh(), 30000);
    }
    return () => {
      listeners.delete(f);
      if (!listeners.size && timer) {
        clearInterval(timer);
        timer = null;
      }
    };
  }, []);
  return { ...shared, reload: () => void refresh() };
}

/** Day, month and year names in the panel's calendar. */
function useDates(settings: UsageSettings | null) {
  const { lang, digitsOf } = useApp();
  const kind = settings?.calendar ?? "jalali";
  const months = cal.MONTHS[kind][lang];
  return {
    kind,
    day: (day: number, year = true) => {
      const { y, m, d } = cal.dateOf(kind, day);
      return digitsOf(lang === "fa" ? `${d} ${months[m - 1]}${year ? ` ${y}` : ""}` : `${months[m - 1]} ${d}${year ? `, ${y}` : ""}`);
    },
    month: (y: number, m: number, year = true) => digitsOf(`${months[m - 1]}${year ? ` ${y}` : ""}`),
    months,
  };
}

// ---- the numbers at the top ----

function Amount({ row, label, strong }: { row: UsageRow | null; label: string; strong?: boolean }) {
  const { num } = useApp();
  const v = row ? volume(total(row)) : null;
  const rx = row ? volume(row.rx) : null;
  const tx = row ? volume(row.tx) : null;
  return (
    <div className={`us-amount ${strong ? "strong" : ""}`}>
      <span className="label">{label}</span>
      <b className="num" dir="ltr">
        {v ? (
          <>
            {num(v.value, v.decimals)} <small>{v.unit}</small>
          </>
        ) : (
          "—"
        )}
      </b>
      {rx && tx && (
        <span className="us-dirs num" dir="ltr">
          <span className="down-c">↓ {num(rx.value, rx.decimals)} {rx.unit}</span> <span className="up-c">↑ {num(tx.value, tx.decimals)} {tx.unit}</span>
        </span>
      )}
    </div>
  );
}

function Amounts({ totals, settings }: { totals: UsageTotals | null; settings: UsageSettings | null }) {
  const { t } = useApp();
  const dates = useDates(settings);
  const since = totals?.since != null && settings ? dates.day(cal.dayAt(settings, totals.since)) : null;
  return (
    <div className="us-amounts">
      <Amount row={totals?.marks[0] ?? null} label={t("us.today")} />
      <Amount row={totals?.marks[1] ?? null} label={t("us.week")} />
      <Amount row={totals?.marks[2] ?? null} label={t("us.month")} />
      <Amount row={totals?.all ?? null} label={since ? t("us.since", { date: since }) : t("us.all")} strong />
    </div>
  );
}

// ---- where the bytes went ----

const PARTS = ["data", "proto", "resend", "headers"] as const;
type Part = (typeof PARTS)[number];

/** A row split into users' data and what the tunnel and the network add. */
export function PartsBar({ row }: { row: UsageRow }) {
  const { t, num } = useApp();
  const percent = usePercent();
  const all = PARTS.reduce((s, p) => s + row[p], 0);
  if (!all) return null;
  const pct = (p: Part) => (row[p] * 100) / all;
  const over = 100 - pct("data");
  return (
    <div className="us-parts">
      <div className="us-parts-head">
        <span>{t("us.parts")}</span>
        <span className="muted small">{t("us.overhead", { pct: percent(over) })}</span>
      </div>
      <div className="us-parts-bar" role="img" aria-label={PARTS.map((p) => `${t(`us.part.${p}`)} ${num(pct(p), 1)}%`).join(", ")}>
        {PARTS.map((p) => (row[p] ? <i key={p} className={`p-${p}`} style={{ flexGrow: row[p] }} /> : null))}
      </div>
      <ul className="us-parts-legend">
        {PARTS.map((p) => {
          const v = volume(row[p]);
          return (
            <li key={p} title={t(`us.part.${p}Help`)}>
              <i className={`p-${p}`} aria-hidden="true" />
              <span>{t(`us.part.${p}`)}</span>
              <b className="num" dir="ltr">
                {num(v.value, v.decimals)} {v.unit}
              </b>
              <span className="muted num" dir="ltr">
                {num(pct(p), pct(p) < 10 ? 1 : 0)}%
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ---- the chart ----

type Range = "today" | "7d" | "30d" | "month" | "year" | "custom";

interface Bar {
  label: string;
  tip: string;
  row: UsageRow;
}

/** What a range asks the panel for, and how its answer becomes bars. */
function plan(range: Range, custom: [number, number], s: UsageSettings) {
  const day = cal.today(s);
  let [from, to] = [day, day]; // days, both included
  if (range === "7d") from = day - 6;
  if (range === "30d") from = day - 29;
  if (range === "month") {
    from = cal.monthStart(s.calendar, day);
    const { y, m } = cal.dateOf(s.calendar, from);
    to = from + cal.monthLen(s.calendar, y, m) - 1;
  }
  if (range === "year") {
    const { y } = cal.dateOf(s.calendar, day);
    from = cal.dayOf(s.calendar, y, 1, 1);
    to = cal.dayOf(s.calendar, y + 1, 1, 1) - 1;
  }
  if (range === "custom") [from, to] = custom;
  const days = to - from + 1;
  const group: "hour" | "day" | "month" = days === 1 ? "hour" : days > 62 ? "month" : "day";
  return { from, to, group, step: group === "hour" ? ("quarter" as const) : ("day" as const) };
}

function UsageChart({ bars, view, unitless }: { bars: Bar[]; view: "dir" | "parts"; unitless?: boolean }) {
  const { t, num, lang } = useApp();
  const [hover, setHover] = useState<number | null>(null);
  const W = 600;
  const H = 170;
  const keys: (keyof UsageRow)[] = view === "dir" ? ["rx", "tx"] : [...PARTS];
  const top = Math.max(...bars.map((b) => keys.reduce((s, k) => s + b.row[k], 0)), 1);
  const slot = W / Math.max(1, bars.length);
  const gap = Math.min(6, slot * 0.25);
  const peak = volume(top);
  const ticks = bars.length <= 12 ? bars.map((_, i) => i) : [0, Math.floor(bars.length / 4), Math.floor(bars.length / 2), Math.floor((3 * bars.length) / 4), bars.length - 1];
  const at = hover !== null ? bars[hover] : null;
  return (
    <div className="chart-block">
      <div className="chart-wrap" dir="ltr">
        <svg
          className="chart us-chart"
          viewBox={`0 0 ${W} ${H}`}
          preserveAspectRatio="none"
          role="img"
          aria-label={t("us.chart")}
          onMouseLeave={() => setHover(null)}
          onMouseMove={(e) => {
            const box = e.currentTarget.getBoundingClientRect();
            const i = Math.floor(((e.clientX - box.left) / box.width) * bars.length);
            setHover(Math.max(0, Math.min(bars.length - 1, i)));
          }}
        >
          {[0.25, 0.5, 0.75].map((k) => (
            <line key={k} className="chart-grid" x1="0" x2={W} y1={H * k} y2={H * k} vectorEffect="non-scaling-stroke" />
          ))}
          {bars.map((b, i) => {
            let y = H;
            return (
              <g key={i} className={hover === i ? "is-hover" : ""}>
                {keys.map((k) => {
                  const h = (b.row[k] / top) * (H - 6);
                  if (h <= 0) return null;
                  y -= h;
                  return <rect key={k} className={`us-${k}`} x={i * slot + gap / 2} y={y} width={Math.max(1, slot - gap)} height={h} rx={Math.min(3, (slot - gap) / 3)} />;
                })}
              </g>
            );
          })}
        </svg>
        {at && hover !== null && (
          <div className="chart-tip is-on us-tip" dir={lang === "fa" ? "rtl" : "ltr"} style={{ left: `clamp(8px, calc(${(((hover + 0.5) * slot) / W) * 100}% - 90px), calc(100% - 190px))` }}>
            <b>{at.tip}</b>
            {keys.map((k) => {
              const v = volume(at.row[k]);
              return (
                <span key={k} className="us-tip-line">
                  <i className={`us-${k}`} aria-hidden="true" />
                  {t(view === "dir" ? `us.${k}` : `us.part.${k}`)}
                  <b className="num">
                    {num(v.value, v.decimals)} {v.unit}
                  </b>
                </span>
              );
            })}
          </div>
        )}
      </div>
      <div className="us-axis" dir="ltr">
        {ticks.map((i) => (
          <span key={i} style={{ left: `${(((i + 0.5) * slot) / W) * 100}%` }}>
            {bars[i]?.label}
          </span>
        ))}
      </div>
      <div className="legend">
        {keys.map((k) => (
          <span key={k} className="us-key">
            <i className={`us-${k}`} aria-hidden="true" />
            {t(view === "dir" ? `us.${k}` : `us.part.${k}`)}
          </span>
        ))}
        {!unitless && (
          <span>
            <Sep />
            {t("chart.max")} {num(peak.value, peak.decimals)} {peak.unit}
          </span>
        )}
      </div>
    </div>
  );
}

// ---- picking days ----

function DayPicker({ settings, from, to, onPick, onClose }: { settings: UsageSettings; from: number; to: number; onPick: (from: number, to: number) => void; onClose: () => void }) {
  const { t, lang, num } = useApp();
  const dates = useDates(settings);
  const kind = settings.calendar;
  const first = cal.dateOf(kind, to);
  const [[y, m], setMonth] = useState<[number, number]>([first.y, first.m]);
  const [a, setA] = useState<number | null>(from);
  const [b, setB] = useState<number | null>(to);
  const today = cal.today(settings);
  const start = cal.dayOf(kind, y, m, 1);
  const len = cal.monthLen(kind, y, m);
  const weekFirst = kind === "jalali" ? 6 : 1;
  const lead = (cal.weekday(start) - weekFirst + 7) % 7;
  const names = Array.from({ length: 7 }, (_, i) => cal.WEEKDAYS[lang][(weekFirst + i) % 7]);
  const lo = a !== null && b !== null ? Math.min(a, b) : a;
  const hi = a !== null && b !== null ? Math.max(a, b) : a;
  const pick = (day: number) => {
    if (a === null || b !== null) {
      setA(day);
      setB(null);
    } else {
      setB(day);
    }
  };
  return (
    <Dialog
      title={t("us.pick")}
      onClose={onClose}
      footer={
        <>
          <span className="muted small grow">{lo !== null ? (hi !== null && hi !== lo ? `${dates.day(lo)} — ${dates.day(hi)}` : dates.day(lo)) : ""}</span>
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={lo === null} onClick={() => lo !== null && onPick(lo, hi ?? lo)}>
            {t("us.show")}
          </button>
        </>
      }
    >
      <p className="small muted" style={{ marginTop: 0 }}>
        {t("us.pickHelp")}
      </p>
      <div className="cal">
        <div className="cal-head">
          <button className="icon-square" type="button" aria-label={t("us.prevMonth")} onClick={() => setMonth(cal.prevMonth(y, m))}>
            <span className="cal-arrow" style={{ transform: lang === "fa" ? undefined : "scaleX(-1)" }}>
              <Icon name="chevron" size={18} />
            </span>
          </button>
          <b>{dates.month(y, m)}</b>
          <button className="icon-square" type="button" aria-label={t("us.nextMonth")} onClick={() => setMonth(cal.nextMonth(y, m))}>
            <span className="cal-arrow" style={{ transform: lang === "fa" ? "scaleX(-1)" : undefined }}>
              <Icon name="chevron" size={18} />
            </span>
          </button>
        </div>
        <div className="cal-grid">
          {names.map((n, i) => (
            <span key={`h${i}`} className="cal-wd" aria-hidden="true">
              {n}
            </span>
          ))}
          {Array.from({ length: lead }, (_, i) => (
            <span key={`e${i}`} />
          ))}
          {Array.from({ length: len }, (_, i) => {
            const day = start + i;
            const inRange = lo !== null && hi !== null && day >= lo && day <= hi;
            const edge = day === lo || day === hi;
            return (
              <button
                key={day}
                type="button"
                className={`cal-day ${inRange ? "in" : ""} ${edge ? "edge" : ""} ${day === today ? "today" : ""}`}
                disabled={day > today}
                aria-pressed={edge}
                aria-label={dates.day(day)}
                onClick={() => pick(day)}
              >
                {num(i + 1)}
              </button>
            );
          })}
        </div>
      </div>
    </Dialog>
  );
}

// ---- the whole panel for one server or tunnel ----

/**
 * Usage of a tunnel (both sides) or a server, with its chart over a chosen range.
 * `sides`: for a tunnel, the servers its sides are on (id, name).
 */
export function UsagePanel({ kind, id, sides }: { kind: "tunnel" | "server"; id: string; sides?: [string, string][] }) {
  const { t, num, digitsOf } = useApp();
  const percent = usePercent();
  const volText = useVolumeText();
  const { settings, summary } = useUsage();
  const dates = useDates(settings);
  const [range, setRange] = useState<Range>("today");
  const [custom, setCustom] = useState<[number, number] | null>(null);
  const [picking, setPicking] = useState(false);
  const [source, setSource] = useState<"card" | "tunnels">("card");
  const [view, setView] = useState<"dir" | "parts">("dir");
  const [rows, setRows] = useState<[number, UsageRow][] | null>(null);
  const of: UsageOf = kind === "tunnel" ? "tunnel" : source === "card" ? "server" : "server-tunnels";
  const parts = of !== "server";
  const shownView = parts ? view : "dir";
  const p = settings ? plan(range, custom ?? [cal.today(settings), cal.today(settings)], settings) : null;
  const key = p ? `${of}|${id}|${p.from}|${p.to}|${p.step}` : "";

  useEffect(() => {
    if (!p || !settings) return;
    let alive = true;
    setRows(null);
    const load = () =>
      api
        .usage(of, id, cal.dayStart(settings, p.from), cal.dayStart(settings, p.to + 1), p.step)
        .then((r) => alive && setRows(r.rows))
        .catch(() => alive && setRows([]));
    void load();
    const timer = setInterval(load, 30000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps

  const bars: Bar[] = useMemo(() => {
    if (!p || !settings || !rows) return [];
    const s = settings;
    if (p.group === "hour") {
      const start = cal.dayStart(s, p.from);
      const out: Bar[] = Array.from({ length: 24 }, (_, h) => ({
        label: digitsOf(String(h).padStart(2, "0")),
        tip: digitsOf(`${String(h).padStart(2, "0")}:00 – ${String(h + 1).padStart(2, "0")}:00`),
        row: ZERO,
      }));
      for (const [ts, r] of rows) {
        const h = Math.floor((ts - start) / 3600);
        if (h >= 0 && h < 24) out[h] = { ...out[h], row: add(out[h].row, r) };
      }
      return out;
    }
    const byDay = new Map<number, UsageRow>();
    for (const [ts, r] of rows) {
      const d = cal.dayAt(s, ts);
      byDay.set(d, add(byDay.get(d) ?? ZERO, r));
    }
    if (p.group === "day") {
      return Array.from({ length: p.to - p.from + 1 }, (_, i) => {
        const d = p.from + i;
        return { label: digitsOf(String(cal.dateOf(s.calendar, d).d)), tip: dates.day(d), row: byDay.get(d) ?? ZERO };
      });
    }
    const out: Bar[] = [];
    let last = "";
    for (let d = p.from; d <= p.to; d++) {
      const { y, m } = cal.dateOf(s.calendar, d);
      const k = `${y}-${m}`;
      if (k !== last) {
        out.push({ label: dates.months[m - 1].slice(0, 3), tip: dates.month(y, m), row: ZERO });
        last = k;
      }
      const b = out[out.length - 1];
      b.row = add(b.row, byDay.get(d) ?? ZERO);
    }
    return out;
  }, [rows, key]); // eslint-disable-line react-hooks/exhaustive-deps

  const inRange = bars.reduce((s, b) => add(s, b.row), ZERO);
  const totals: UsageTotals | null = summary ? (kind === "tunnel" ? summary.tunnels[id] : summary.servers[id]?.own) ?? null : null;
  const serverTunnels = kind === "server" ? summary?.servers[id]?.tunnels ?? null : null;
  const rangeLabel = p && settings ? (p.from === p.to ? dates.day(p.from) : `${dates.day(p.from, false)} — ${dates.day(p.to)}`) : "";
  const v = volume(total(inRange));
  const estPct = total(inRange) ? (inRange.est * 100) / total(inRange) : 0;

  return (
    <div className="usage">
      <Amounts totals={totals} settings={settings} />

      {kind === "tunnel" && sides && sides.length > 0 && summary?.sides[id] && (
        <div className="us-sides">
          <span className="label">{t("us.sides")}</span>
          {sides.map(([sid, name]) => {
            const side = summary.sides[id]?.[sid];
            const sv = volume(side ? total(side.all) : 0);
            return (
              <span key={sid} className="us-side">
                {name}
                <b className="num" dir="ltr">
                  {num(sv.value, sv.decimals)} {sv.unit}
                </b>
              </span>
            );
          })}
          <span className="help">{t("us.sidesHelp")}</span>
        </div>
      )}

      {kind === "server" && serverTunnels && totals && (
        <ShareLine own={totals.marks[2] ?? ZERO} tunnels={serverTunnels.marks[2] ?? ZERO} />
      )}

      <div className="us-controls">
        <Seg
          value={range}
          options={(["today", "7d", "30d", "month", "year"] as Range[]).map((r) => [r, t(`us.range.${r}`)] as [Range, string]).concat([["custom", custom && range === "custom" ? rangeLabel : t("us.range.custom")]])}
          onChange={(r) => (r === "custom" ? setPicking(true) : setRange(r))}
        />
        <span className="grow" />
        {kind === "server" && (
          <Seg
            value={source}
            options={[
              ["card", t("us.src.card")],
              ["tunnels", t("us.src.tunnels")],
            ]}
            onChange={setSource}
          />
        )}
        {parts && (
          <Seg
            value={view}
            options={[
              ["dir", t("us.view.dir")],
              ["parts", t("us.view.parts")],
            ]}
            onChange={setView}
          />
        )}
      </div>

      <div className="us-range-head">
        <span className="muted small">{rangeLabel}</span>
        <b className="num" dir="ltr">
          {num(v.value, v.decimals)} <small>{v.unit}</small>
        </b>
      </div>

      {rows === null ? (
        <div className="us-chart-wait" aria-hidden="true" />
      ) : total(inRange) === 0 ? (
        <p className="muted small us-none">{t("us.none")}</p>
      ) : (
        <UsageChart bars={bars} view={shownView} />
      )}

      {parts && total(inRange) > 0 && <PartsBar row={inRange} />}

      {kind === "tunnel" && total(inRange) > 0 && (
        <p className="help">{t("us.userBytes", { down: volText(inRange.down), up: volText(inRange.up) })}</p>
      )}

      {estPct > 0.05 && (
        <div className="banner warn small" role="note">
          <Icon name="alert" size={18} />
          <span>{t("us.est", { pct: percent(estPct) })}</span>
        </div>
      )}

      {picking && settings && (
        <DayPicker
          settings={settings}
          from={p?.from ?? cal.today(settings)}
          to={p?.to ?? cal.today(settings)}
          onClose={() => setPicking(false)}
          onPick={(a, b) => {
            setCustom([a, b]);
            setRange("custom");
            setPicking(false);
          }}
        />
      )}
    </div>
  );
}

/** A server's month: how much of it its tunnels were. */
function ShareLine({ own, tunnels }: { own: UsageRow; tunnels: UsageRow }) {
  const { t, num } = useApp();
  const all = total(own);
  if (!all) return null;
  const tun = Math.min(total(tunnels), all);
  const pct = (tun * 100) / all;
  const tv = volume(tun);
  const ov = volume(all - tun);
  return (
    <div className="us-share">
      <div className="us-parts-bar">
        {tun > 0 && <i className="p-data" style={{ flexGrow: tun }} />}
        {all - tun > 0 && <i className="p-other" style={{ flexGrow: all - tun }} />}
      </div>
      <div className="us-share-legend small">
        <span>
          <i className="p-data" aria-hidden="true" />
          {t("us.monthTunnels")} <b className="num" dir="ltr">{num(tv.value, tv.decimals)} {tv.unit}</b> <span className="muted num" dir="ltr">{num(pct, 0)}%</span>
        </span>
        <span>
          <i className="p-other" aria-hidden="true" />
          {t("us.monthOther")} <b className="num" dir="ltr">{num(ov.value, ov.decimals)} {ov.unit}</b>
        </span>
      </div>
    </div>
  );
}

// ---- quota ----

/** How much of a server's monthly quota is used, and until when. */
export function QuotaBar({ quota, settings, compact }: { quota: QuotaState; settings: UsageSettings | null; compact?: boolean }) {
  const { t, num } = useApp();
  const percent = usePercent();
  const dates = useDates(settings);
  const pct = Math.min(100, (quota.used * 100) / quota.limit);
  const tone = quota.used >= quota.limit ? "full" : pct >= quota.warn ? "warn" : "";
  const used = volume(quota.used);
  const limit = volume(quota.limit);
  const renews = settings ? dates.day(cal.dayAt(settings, quota.period_end), false) : "";
  return (
    <div className={`us-quota ${tone} ${compact ? "compact" : ""}`}>
      <div className="us-quota-head small">
        <span>{t("us.quota")}</span>
        <span className="num" dir="ltr">
          {num(used.value, used.decimals)} {used.unit} / {num(limit.value, limit.decimals)} {limit.unit}
        </span>
      </div>
      <div className="us-quota-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label={t("us.quota")}>
        <i style={{ width: `${pct}%` }} />
        <b style={{ insetInlineStart: `${quota.warn}%` }} aria-hidden="true" />
      </div>
      {!compact && <span className="muted small">{t("us.renews", { date: renews, pct: percent(pct) })}</span>}
    </div>
  );
}

const GB = 1024 ** 3;

function QuotaDialog({ server, name, quota, onClose, onSaved }: { server: string; name: string; quota: QuotaState | null; onClose: () => void; onSaved: () => void }) {
  const { t, toast } = useApp();
  const tb = quota ? quota.limit >= 1024 * GB && quota.limit % (1024 * GB) === 0 : true;
  const [unit, setUnit] = useState<"GB" | "TB">(tb ? "TB" : "GB");
  const [amount, setAmount] = useState(quota ? String(quota.limit / (tb ? 1024 * GB : GB)) : "2");
  const [count, setCount] = useState<Quota["count"]>(quota?.count ?? "both");
  const [day, setDay] = useState(quota?.reset_day ?? 1);
  const [warn, setWarn] = useState(String(quota?.warn ?? 80));
  const [busy, setBusy] = useState(false);
  const save = async (remove: boolean) => {
    const value = Number(amount.replace(/[۰-۹]/g, (c) => String("۰۱۲۳۴۵۶۷۸۹".indexOf(c))));
    const w = Number(warn.replace(/[۰-۹]/g, (c) => String("۰۱۲۳۴۵۶۷۸۹".indexOf(c))));
    const limit = remove ? 0 : Math.round(value * (unit === "TB" ? 1024 * GB : GB));
    if (!remove && (!(value > 0) || !(w >= 1 && w <= 99))) {
      toast(t("us.quotaBad"), "err");
      return;
    }
    setBusy(true);
    try {
      await api.usageQuota(server, { limit, count, reset_day: day, warn: w });
      toast(t(remove ? "us.quotaRemoved" : "us.quotaSaved", { name }), "ok");
      onSaved();
      onClose();
    } catch {
      toast(t("us.quotaBad"), "err");
    }
    setBusy(false);
  };
  return (
    <Dialog
      title={t("us.quotaTitle", { name })}
      onClose={onClose}
      footer={
        <>
          {quota && (
            <button className="btn btn-quiet btn-sm" type="button" disabled={busy} onClick={() => save(true)}>
              <Icon name="trash" size={16} />
              {t("us.quotaRemove")}
            </button>
          )}
          <span className="grow" />
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={busy} onClick={() => save(false)}>
            {t("us.save")}
          </button>
        </>
      }
    >
      <p className="small muted" style={{ marginTop: 0 }}>
        {t("us.quotaHelp")}
      </p>
      <div className="field">
        <label htmlFor="q-amount">{t("us.quotaLimit")}</label>
        <div className="us-row">
          <input id="q-amount" className="text mono" inputMode="decimal" dir="ltr" value={amount} onChange={(e) => setAmount(e.target.value)} />
          <Seg value={unit} options={[["GB", "GB"], ["TB", "TB"]]} onChange={setUnit} />
        </div>
      </div>
      <div className="field">
        <span className="label">{t("us.quotaCount")}</span>
        <Seg
          value={count}
          options={[
            ["both", t("us.count.both")],
            ["tx", t("us.count.tx")],
            ["rx", t("us.count.rx")],
          ]}
          onChange={setCount}
        />
        <span className="help">{t("us.quotaCountHelp")}</span>
      </div>
      <div className="grid-2">
        <div className="field">
          <label htmlFor="q-day">{t("us.quotaDay")}</label>
          <select id="q-day" className="select" value={day} onChange={(e) => setDay(Number(e.target.value))}>
            {Array.from({ length: 31 }, (_, i) => (
              <option key={i + 1} value={i + 1}>
                {i + 1}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor="q-warn">{t("us.quotaWarn")}</label>
          <input id="q-warn" className="text mono" inputMode="numeric" dir="ltr" value={warn} onChange={(e) => setWarn(e.target.value)} />
        </div>
      </div>
    </Dialog>
  );
}

// ---- reset ----

function ResetDialog({ kind, id, name, onClose, onDone }: { kind: "tunnel" | "server"; id: string; name: string; onClose: () => void; onDone: () => void }) {
  const { t, toast } = useApp();
  const [tunnels, setTunnels] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <Dialog
      title={t("us.resetTitle", { name })}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button
            className="btn btn-danger solid btn-sm"
            type="button"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api.usageReset(kind === "tunnel" ? { tunnel: id } : { server: id, tunnels });
                toast(t("us.resetDone", { name }), "ok");
                onDone();
                onClose();
              } catch {
                toast(t("bak.failed"), "err");
              }
              setBusy(false);
            }}
          >
            {t("us.reset")}
          </button>
        </>
      }
    >
      <p className="small" style={{ marginTop: 0 }}>
        {t(kind === "tunnel" ? "us.resetTextTunnel" : "us.resetText")}
      </p>
      {kind === "server" && (
        <label className="check">
          <input type="checkbox" checked={tunnels} onChange={(e) => setTunnels(e.target.checked)} />
          <span>{t("us.resetTunnels")}</span>
        </label>
      )}
    </Dialog>
  );
}

/** The reset button of a tunnel's or server's usage, with its confirmation. */
export function ResetUsage({ kind, id, name }: { kind: "tunnel" | "server"; id: string; name: string }) {
  const { t } = useApp();
  const { reload } = useUsage();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button className="btn btn-quiet btn-sm" type="button" onClick={() => setOpen(true)}>
        <Icon name="restart" size={16} />
        {t("us.reset")}
      </button>
      {open && <ResetDialog kind={kind} id={id} name={name} onClose={() => setOpen(false)} onDone={reload} />}
    </>
  );
}

/** A server's usage in a dialog of its own: its chart, its quota, its reset. */
export function ServerUsageDialog({ id, name, onClose }: { id: string; name: string; onClose: () => void }) {
  const { t } = useApp();
  const { settings, summary, reload } = useUsage();
  const [editing, setEditing] = useState(false);
  const quota = summary?.servers[id]?.quota ?? null;
  return (
    <Dialog title={t("us.serverTitle", { name })} onClose={onClose} wide>
      <p className="small muted" style={{ marginTop: 0 }}>
        {t("us.serverSub")}
      </p>
      <div className="us-quota-row">
        {quota ? <QuotaBar quota={quota} settings={settings} /> : <span className="muted small grow">{t("us.quotaNone")}</span>}
        <button className="btn btn-ghost btn-sm" type="button" onClick={() => setEditing(true)}>
          <Icon name={quota ? "edit" : "plus"} size={16} />
          {t(quota ? "us.quotaEdit" : "us.quotaSet")}
        </button>
        <ResetUsage kind="server" id={id} name={name} />
      </div>
      <UsagePanel kind="server" id={id} />
      {editing && <QuotaDialog server={id} name={name} quota={quota} onClose={() => setEditing(false)} onSaved={reload} />}
    </Dialog>
  );
}

/** Calendar and time zone of usage, for the settings page. */
export function UsageSettingsRows({ row }: { row: (title: string, text: string, control: React.ReactNode) => React.ReactNode }) {
  const { t, toast } = useApp();
  const { settings, reload } = useUsage();
  if (!settings) return null;
  const set = async (s: UsageSettings) => {
    try {
      await api.setUsageSettings(s);
      toast(t("us.settingsSaved"), "ok");
      reload();
    } catch {
      toast(t("bak.failed"), "err");
    }
  };
  const zone = (o: number) => `UTC${o < 0 ? "−" : "+"}${String(Math.floor(Math.abs(o) / 60)).padStart(2, "0")}:${String(Math.abs(o) % 60).padStart(2, "0")}`;
  const zones = [-480, -300, -240, 0, 60, 120, 180, 210, 240, 270, 300, 330, 345, 360, 420, 480, 540];
  if (!zones.includes(settings.offset)) zones.push(settings.offset);
  zones.sort((a, b) => a - b);
  return (
    <>
      {row(
        t("set.calendar"),
        t("set.calendarText"),
        <Seg
          value={settings.calendar}
          options={[
            ["jalali", t("cal.jalali")],
            ["gregorian", t("cal.gregorian")],
          ]}
          onChange={(c) => set({ ...settings, calendar: c })}
        />,
      )}
      {row(
        t("set.tz"),
        t("set.tzText"),
        <select className="select" aria-label={t("set.tz")} value={settings.offset} onChange={(e) => set({ ...settings, offset: Number(e.target.value) })} dir="ltr" style={{ maxWidth: 240 }}>
          {zones.map((o) => (
            <option key={o} value={o}>
              {zone(o)}
              {o === 210 ? ` · ${t("set.tzTehran")}` : ""}
            </option>
          ))}
        </select>,
      )}
    </>
  );
}

/** A server card's line: this month's usage, and its quota when it has one. */
export function ServerUsageLine({ id }: { id: string }) {
  const { t, num } = useApp();
  const { summary, settings } = useUsage();
  const s = summary?.servers[id];
  const month = s?.own.marks[2];
  const v = month ? volume(total(month)) : null;
  return (
    <div className="srv-usage">
      <div className="srv-usage-line">
        <span className="label">{t("us.cardMonth")}</span>
        <b className="num" dir="ltr">
          {v ? (
            <>
              {num(v.value, v.decimals)} <small>{v.unit}</small>
            </>
          ) : (
            "—"
          )}
        </b>
        {month && total(month) > 0 && (
          <span className="num small" dir="ltr">
            <span className="down-c">↓ {num(volume(month.rx).value, volume(month.rx).decimals)} {volume(month.rx).unit}</span>{" "}
            <span className="up-c">↑ {num(volume(month.tx).value, volume(month.tx).decimals)} {volume(month.tx).unit}</span>
          </span>
        )}
      </div>
      {s?.quota && <QuotaBar quota={s.quota} settings={settings} compact />}
    </div>
  );
}
