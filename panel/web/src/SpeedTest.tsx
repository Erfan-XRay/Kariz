// The speed test of a tunnel: water through the channel while it measures, and a gauge
// that fills to what was measured. Nothing is made up: while the test runs the gauge only
// shows which phase it is in (the phases have known lengths); the numbers come at the end,
// from the entry server's report.
import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { SpeedReport, SpeedResult } from "./api";
import { clamp, fitCanvas, paletteNow, rgba } from "./draw";
import type { Tunnel } from "./derive";
import { useApp } from "./store";
import { Icon, Seg, useAgo } from "./ui";

type Phase = "ready" | "ping" | "down" | "up" | "udp" | "done" | "failed";
const STEPS = ["ping", "down", "up", "udp"] as const;
type Step = (typeof STEPS)[number];

const SCALES = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];
const scaleFor = (mbps: number) => SCALES.find((s) => s >= mbps * 1.1) ?? SCALES[SCALES.length - 1];

/** How long each phase lasts, as the core runs them (`speedtest::run`). */
function plan(seconds: number, udp: boolean): [Step, number][] {
  return [
    ["ping", Math.min(seconds, 2)],
    ["down", seconds + 0.5],
    ["up", seconds + 0.5],
    ...(udp ? ([["udp", Math.min(seconds, 5)]] as [Step, number][]) : []),
  ];
}

interface Last {
  at: number;
  d: number;
  u: number;
}

const lastKey = (name: string) => `kariz.speed.${name}`;
function readLast(name: string): Last | null {
  try {
    const v = JSON.parse(localStorage.getItem(lastKey(name)) ?? "null");
    return v && typeof v.d === "number" ? v : null;
  } catch {
    return null;
  }
}
function saveLast(name: string, r: SpeedReport) {
  try {
    localStorage.setItem(lastKey(name), JSON.stringify({ at: Date.now(), d: r.download.mbps, u: r.upload.mbps }));
  } catch {
    // private window: no memory of the last test, nothing else changes
  }
}

/** A number that runs up to `target` when it changes. */
function useCountUp(target: number, ms: number, low: boolean): number {
  const [v, setV] = useState(target);
  const from = useRef(target);
  useEffect(() => {
    if (low) {
      setV(target);
      from.current = target;
      return;
    }
    const start = performance.now();
    const a = from.current;
    let raf = 0;
    const step = (now: number) => {
      const k = clamp((now - start) / ms, 0, 1);
      const e = 1 - Math.pow(1 - k, 4);
      const x = a + (target - a) * e;
      setV(x);
      from.current = x;
      if (k < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [target, ms, low]);
  return v;
}

/** The arc of the gauge: 240 degrees, open at the bottom. */
const ARC = { cx: 160, cy: 150, start: 150, sweep: 240 };
function arcPath(r: number): string {
  const p = (deg: number) => {
    const a = (deg * Math.PI) / 180;
    return `${(ARC.cx + r * Math.cos(a)).toFixed(2)} ${(ARC.cy + r * Math.sin(a)).toFixed(2)}`;
  };
  return `M${p(ARC.start)} A${r} ${r} 0 1 1 ${p(ARC.start + ARC.sweep)}`;
}
function onArc(r: number, frac: number) {
  const a = ((ARC.start + ARC.sweep * frac) * Math.PI) / 180;
  return { x: ARC.cx + r * Math.cos(a), y: ARC.cy + r * Math.sin(a) };
}

function Gauge({ phase, down, up, scale }: { phase: Phase; down: number; up: number; scale: number }) {
  const { t, num, low } = useApp();
  const d = useCountUp(down, 1600, low);
  const u = useCountUp(up, 1900, low);
  const running = phase !== "ready" && phase !== "done" && phase !== "failed";
  const fd = clamp(d / scale, 0, 1);
  const fu = clamp(u / scale, 0, 1);
  const tip = onArc(118, fd);
  const ticks = Array.from({ length: 21 }, (_, i) => i / 20);
  const dp = down >= 100 ? 0 : 1;
  return (
    <svg className={`sp-gauge ${running ? "is-running" : ""} phase-${phase}`} viewBox="0 -26 320 262" aria-hidden="true">
      <defs>
        <linearGradient id="sp-water" x1="0" x2="1" y1="1" y2="0">
          <stop offset="0" stopColor="var(--water-deep)" />
          <stop offset=".55" stopColor="var(--water)" />
          <stop offset="1" stopColor="var(--water-bright)" />
        </linearGradient>
        <filter id="sp-glow" x="-50%" y="-50%" width="200%" height="200%">
          <feGaussianBlur stdDeviation="5" />
        </filter>
      </defs>
      {ticks.map((k) => {
        const a = onArc(136, k);
        const b = onArc(k % 0.25 === 0 ? 127 : 131, k);
        return <line key={k} className={`sp-tick ${k <= fd && fd > 0 ? "on" : ""}`} x1={a.x} y1={a.y} x2={b.x} y2={b.y} />;
      })}
      {[0, 0.25, 0.5, 0.75, 1].map((k) => {
        const p = onArc(150, k);
        return (
          <text key={k} className="sp-scale" x={p.x} y={p.y + 4} textAnchor="middle">
            {num(scale * k, 0)}
          </text>
        );
      })}
      <path className="sp-track" d={arcPath(118)} pathLength={1} />
      <path className="sp-track thin" d={arcPath(100)} pathLength={1} />
      {running && <path className="sp-sweep" d={arcPath(118)} pathLength={1} />}
      {running && <path className="sp-flow" d={arcPath(118)} pathLength={1} />}
      {fd > 0.001 && <path className="sp-down" d={arcPath(118)} pathLength={1} style={{ strokeDashoffset: 1 - fd }} />}
      {fu > 0.001 && <path className="sp-up" d={arcPath(100)} pathLength={1} style={{ strokeDashoffset: 1 - fu }} />}
      {fd > 0.002 && (
        <>
          <circle className="sp-tip-glow" cx={tip.x} cy={tip.y} r="10" filter="url(#sp-glow)" />
          <circle className="sp-tip" cx={tip.x} cy={tip.y} r="5" />
        </>
      )}
      <text className="sp-label" x={ARC.cx} y={98} textAnchor="middle">
        {running ? t(`sp.phase.${phase}`) : phase === "done" ? t("sp.phase.done") : t("sp.ready")}
      </text>
      <text className="sp-value" x={ARC.cx} y={150} textAnchor="middle">
        {running ? "···" : phase === "done" ? num(d, dp) : "—"}
      </text>
      <text className="sp-unit" x={ARC.cx} y={176} textAnchor="middle">
        {running ? t("sp.measuring") : "Mbps ↓"}
      </text>
      {phase === "done" && (
        <text className="sp-second" x={ARC.cx} y={206} textAnchor="middle">
          ↑ {num(u, up >= 100 ? 0 : 1)} Mbps
        </text>
      )}
    </svg>
  );
}

/** Water through a channel behind the gauge: its direction and density follow the phase. */
function Flow({ phase, level }: { phase: Phase; level: number }) {
  const { low } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const state = useRef({ phase, level });
  state.current = { phase, level };

  useEffect(() => {
    const cv = canvas.current;
    if (!cv) return;
    type Drop = { x: number; y: number; v: number; r: number; a: number };
    let drops: Drop[] = [];
    let raf = 0;
    let last = performance.now();
    let ping = 0;
    const draw = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      const P = paletteNow();
      const { ctx, W, H } = fitCanvas(cv);
      const rtl = document.documentElement.dir === "rtl";
      const { phase: ph, level: lv } = state.current;
      ctx.clearRect(0, 0, W, H);
      const mid = H * 0.62;
      const band = Math.max(26, H * 0.14);
      // the channel: a dark bed with a faint water line
      const bed = ctx.createLinearGradient(0, mid - band, 0, mid + band);
      bed.addColorStop(0, rgba(P.water, 0));
      bed.addColorStop(0.5, rgba(P.water, P.night ? 0.1 : 0.14));
      bed.addColorStop(1, rgba(P.water, 0));
      ctx.fillStyle = bed;
      ctx.fillRect(0, mid - band, W, band * 2);
      // which way the water goes: download runs towards the entry (the inline start)
      const toStart = ph === "down" || ph === "done" || ph === "ready";
      const dir = (toStart ? -1 : 1) * (rtl ? -1 : 1);
      const busy = ph === "down" || ph === "up";
      const want = low ? 0 : busy ? 160 : ph === "udp" ? 50 : ph === "ping" ? 0 : ph === "done" ? Math.round(30 + 90 * lv) : 22;
      const speed = busy ? 520 : ph === "udp" ? 320 : ph === "done" ? 90 + 300 * lv : 50;
      while (drops.length < want) {
        drops.push({
          x: Math.random() * W,
          y: mid + (Math.random() - 0.5) * band * 1.4,
          v: 0.6 + Math.random() * 0.8,
          r: 2 + Math.random() * 5,
          a: 0.35 + Math.random() * 0.65,
        });
      }
      drops.length = want;
      ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
      for (const d of drops) {
        d.x += dir * speed * d.v * dt;
        if (d.x < -20) d.x = W + 20;
        if (d.x > W + 20) d.x = -20;
        const tail = busy ? 26 * d.v : 0;
        if (tail) {
          const g = ctx.createLinearGradient(d.x, 0, d.x - dir * tail, 0);
          g.addColorStop(0, rgba(P.waterBright, 0.55 * d.a));
          g.addColorStop(1, rgba(P.waterBright, 0));
          ctx.strokeStyle = g;
          ctx.lineWidth = d.r * 0.7;
          ctx.lineCap = "round";
          ctx.beginPath();
          ctx.moveTo(d.x, d.y);
          ctx.lineTo(d.x - dir * tail, d.y);
          ctx.stroke();
        }
        ctx.globalAlpha = d.a;
        ctx.drawImage(P.sprite, d.x - d.r * 2, d.y - d.r * 2, d.r * 4, d.r * 4);
        ctx.globalAlpha = 1;
      }
      // the latency phase: one drop to the far end and back
      if (ph === "ping" && !low) {
        ping = (ping + dt * 1.6) % 2;
        const k = ping < 1 ? ping : 2 - ping;
        const x = rtl ? W * (0.92 - 0.84 * k) : W * (0.08 + 0.84 * k);
        for (let i = 0; i < 3; i++) {
          ctx.globalAlpha = 1 - i * 0.3;
          const r = 9 - i * 2;
          ctx.drawImage(P.sprite, x - (ping < 1 ? 1 : -1) * (rtl ? -1 : 1) * i * 10 - r * 2, mid - r * 2, r * 4, r * 4);
        }
        ctx.globalAlpha = 1;
      }
      ctx.globalCompositeOperation = "source-over";
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [low]);

  return <canvas ref={canvas} className="sp-flow-canvas" aria-hidden="true" />;
}

export function SpeedTest({ tunnel }: { tunnel: Tunnel }) {
  const { t, num, low } = useApp();
  const ago = useAgo();
  const [seconds, setSeconds] = useState(10);
  const [streams, setStreams] = useState(4);
  const [udp, setUdp] = useState(true);
  const [phase, setPhase] = useState<Phase>("ready");
  const [result, setResult] = useState<SpeedResult | null>(null);
  const [error, setError] = useState("");
  const [last, setLast] = useState<Last | null>(() => readLast(tunnel.name));
  const [elapsed, setElapsed] = useState(0);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const running = phase !== "ready" && phase !== "done" && phase !== "failed";
  const steps = plan(seconds, udp);
  const total = steps.reduce((s, [, d]) => s + d, 0);

  // While it runs: which phase it is in, from the known lengths of the phases.
  useEffect(() => {
    if (!running) return;
    const start = performance.now();
    const id = setInterval(() => {
      const e = (performance.now() - start) / 1000;
      setElapsed(e);
      let acc = 0;
      let now: Step = steps[steps.length - 1][0];
      for (const [s, d] of steps) {
        acc += d;
        if (e < acc) {
          now = s;
          break;
        }
      }
      setPhase((p) => (p === "done" || p === "failed" ? p : now));
    }, 200);
    return () => clearInterval(id);
  }, [running && "on"]);

  const go = async () => {
    setError("");
    setResult(null);
    setElapsed(0);
    setPhase("ping");
    try {
      const r = await api.speedtest(tunnel.name, seconds, streams, udp);
      if (!alive.current) return;
      setResult(r);
      if (!r.ok) {
        setError(r.error ?? t("speed.failed"));
        setPhase("failed");
        return;
      }
      if (r.report) {
        setLast(readLast(tunnel.name));
        saveLast(tunnel.name, r.report);
      }
      setPhase("done");
    } catch (e) {
      if (!alive.current) return;
      setError(e instanceof ApiError ? e.code : String(e));
      setPhase("failed");
    }
  };

  const report = result?.report ?? null;
  const down = report?.download.mbps ?? 0;
  const up = report?.upload.mbps ?? 0;
  const scale = scaleFor(Math.max(down, up, report ? 0 : Math.max(tunnel.rate, 50)));
  const level = clamp(Math.log10(1 + down) / 4, 0, 1);
  const fmt = (v: number) => num(v, v >= 100 ? 0 : 1);
  const stepState = (s: Step) => {
    if (phase === "done") return s === "udp" && !report?.udp ? "skip" : "done";
    if (!running) return "wait";
    const at = STEPS.indexOf(phase as Step);
    const i = STEPS.indexOf(s);
    if (!udp && s === "udp") return "skip";
    return i < at ? "done" : i === at ? "active" : "wait";
  };
  const stepValue = (s: Step): string => {
    if (!report) return "";
    if (s === "ping") return `${num(report.idle.p50_ms, 0)} ms`;
    if (s === "down") return `${fmt(report.download.mbps)} Mbps`;
    if (s === "up") return `${fmt(report.upload.mbps)} Mbps`;
    return report.udp ? `${num(lossOf(report.udp), 1)}%` : t("sp.noUdp");
  };
  const canRun = tunnel.state === "up" && !!tunnel.entry;

  return (
    <div className={`speed ${low ? "is-low" : ""}`}>
      <div className="sp-stage">
        <Flow phase={phase} level={level} />
        <Gauge phase={phase} down={down} up={up} scale={scale} />
        {running && (
          <div className="sp-progress" aria-hidden="true">
            <i style={{ width: `${clamp((elapsed / total) * 100, 0, 100)}%` }} />
          </div>
        )}
      </div>

      <ol className="sp-steps" aria-label={t("sp.title")}>
        {STEPS.map((s) => (
          <li key={s} className={`sp-step ${stepState(s)}`} aria-current={stepState(s) === "active" ? "step" : undefined}>
            <span className="sp-dot">{stepState(s) === "done" ? <Icon name="check" size={14} /> : null}</span>
            <span className="sp-name">{t(`sp.phase.${s}`)}</span>
            <span className="sp-val num">{stepValue(s)}</span>
          </li>
        ))}
      </ol>

      <div className="sp-live" role="status" aria-live="polite">
        {phase === "done" && report ? `↓ ${fmt(down)} Mbps, ↑ ${fmt(up)} Mbps` : running ? t(`sp.phase.${phase}`) : ""}
      </div>

      {phase === "done" && report && (
        <div className="sp-results">
          <div className="sp-card down">
            <span className="label">↓ {t("sp.phase.down")}</span>
            <b className="num">{fmt(report.download.mbps)}</b>
            <small>Mbps · {t("sp.peak", { v: fmt(report.download.peak_mbps) })}</small>
          </div>
          <div className="sp-card up">
            <span className="label">↑ {t("sp.phase.up")}</span>
            <b className="num">{fmt(report.upload.mbps)}</b>
            <small>Mbps · {t("sp.peak", { v: fmt(report.upload.peak_mbps) })}</small>
          </div>
          <div className="sp-card">
            <span className="label">{t("sp.ping")}</span>
            <b className="num">{num(report.idle.p50_ms, 0)}</b>
            <small>ms · p99 {num(report.idle.p99_ms, 0)}</small>
          </div>
          <div className="sp-card">
            <span className="label">{t("sp.loaded")}</span>
            <b className="num">{num(Math.max(report.download_latency.p50_ms, report.upload_latency.p50_ms), 0)}</b>
            <small>ms</small>
          </div>
          <div className="sp-card">
            <span className="label">{t("sp.jitter")}</span>
            <b className="num">{num(report.idle.jitter_ms, 1)}</b>
            <small>ms</small>
          </div>
          <div className="sp-card">
            <span className="label">{t("sp.loss")}</span>
            <b className="num">{report.udp ? num(lossOf(report.udp), 1) : "—"}</b>
            <small>{report.udp ? "%" : t("sp.noUdp")}</small>
          </div>
        </div>
      )}
      {phase === "done" && report?.notes.map((n) => (
        <p key={n} className="muted small">
          {n}
        </p>
      ))}
      {phase === "done" && result && !report && (
        <>
          <p className="muted small">{t("sp.old")}</p>
          <pre className="code" dir="ltr">
            {result.text}
          </pre>
        </>
      )}
      {phase === "failed" && <p className="err small">{t("sp.failed", { why: error })}</p>}

      <div className="sp-controls">
        <div className="sp-opt">
          <span className="label">{t("sp.length")}</span>
          <Seg value={String(seconds)} options={["5", "10", "20", "30"].map((v) => [v, `${num(+v)}s`] as [string, string])} onChange={(v) => !running && setSeconds(+v)} />
        </div>
        <div className="sp-opt">
          <span className="label">{t("sp.streams")}</span>
          <Seg value={String(streams)} options={["1", "4", "8"].map((v) => [v, num(+v)] as [string, string])} onChange={(v) => !running && setStreams(+v)} />
        </div>
        <label className="check sp-opt">
          <input type="checkbox" checked={udp} disabled={running} onChange={(e) => setUdp(e.target.checked)} /> {t("sp.udp")}
        </label>
        <span className="grow" />
        <button className="btn btn-primary sp-go" type="button" disabled={!canRun || running} onClick={() => void go()}>
          <span className="shine" />
          <Icon name="bolt" size={18} />
          {phase === "done" || phase === "failed" ? t("sp.again") : t("sp.go")}
        </button>
      </div>
      <p className="muted small sp-note">
        {canRun ? t("sp.idle") : t("sp.off")}
        {last && ` ${t("sp.last", { ago: ago((Date.now() - last.at) / 1000), d: fmt(last.d), u: fmt(last.u) })}`}
      </p>
    </div>
  );
}

function lossOf(l: { sent: number; received: number }): number {
  return l.sent ? (100 * Math.max(0, l.sent - Math.min(l.received, l.sent))) / l.sent : 0;
}
