// The speed test of a tunnel: water through the channel while it measures, and a gauge
// that fills to what is measured. Nothing is made up: the entry server runs the test in the
// background and the panel follows its progress lines (the phase it is in, a rate twice a
// second, the result of each phase as it ends). An agent older than 1.5 cannot be followed:
// the test then runs as one request and the numbers come at the end.
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

/** Ready-made settings. Max opens the most streams for long enough to fill the tunnel. */
const PRESETS = {
  quick: { seconds: 5, streams: 4 },
  standard: { seconds: 10, streams: 4 },
  max: { seconds: 20, streams: 16 },
} as const;
type Preset = keyof typeof PRESETS | "custom";
const presetOf = (seconds: number, streams: number): Preset =>
  (Object.keys(PRESETS) as (keyof typeof PRESETS)[]).find((k) => PRESETS[k].seconds === seconds && PRESETS[k].streams === streams) ?? "custom";

/** What a running test has reported so far. */
interface Partial {
  idle?: { p50_ms: number; p99_ms: number; jitter_ms: number };
  download?: { mbps: number; peak_mbps: number };
  upload?: { mbps: number; peak_mbps: number };
  udp?: { sent: number; received: number };
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
  // While a transfer runs the number is the rate right now.
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
      {/* A plate under the numbers: the water behind the gauge never runs through them. */}
      <circle className="sp-plate" cx={ARC.cx} cy={150} r="88" />
      <text className="sp-label" x={ARC.cx} y={98} textAnchor="middle">
        {running ? t(`sp.phase.${phase}`) : phase === "done" ? t("sp.phase.done") : t("sp.ready")}
      </text>
      <text className="sp-value" x={ARC.cx} y={150} textAnchor="middle">
        {running ? (phase === "down" && d > 0 ? num(d, dp) : phase === "up" && u > 0 ? num(u, up >= 100 ? 0 : 1) : "···") : phase === "done" ? num(d, dp) : "—"}
      </text>
      <text className="sp-unit" x={ARC.cx} y={176} textAnchor="middle">
        {running ? (phase === "down" ? "Mbps ↓" : phase === "up" ? "Mbps ↑" : t("sp.measuring")) : "Mbps ↓"}
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
  // The rate right now, what each phase has measured so far, and the rates of this phase.
  const [live, setLive] = useState(0);
  const [partial, setPartial] = useState<Partial>({});
  const [samples, setSamples] = useState<number[]>([]);
  const [best, setBest] = useState(0);
  const [custom, setCustom] = useState(false);
  const [result, setResult] = useState<SpeedResult | null>(null);
  const [error, setError] = useState("");
  const [last, setLast] = useState<Last | null>(() => readLast(tunnel.name));
  const [elapsed, setElapsed] = useState(0);
  const alive = useRef(true);
  // The test that runs, to stop it: by the button, or when this page is left.
  const job = useRef<string | null>(null);
  const live_ = useRef(false);
  const [stopping, setStopping] = useState(false);
  const [stopped, setStopped] = useState(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (job.current && live_.current) void api.speedtestStop(tunnel.name, job.current).catch(() => {});
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const running = phase !== "ready" && phase !== "done" && phase !== "failed";
  live_.current = running;
  const steps = plan(seconds, udp);
  const total = steps.reduce((s, [, d]) => s + d, 0);

  // The time spent, for the bar under the gauge (the phase itself comes from the test).
  useEffect(() => {
    if (!running) return;
    const start = performance.now();
    const id = setInterval(() => setElapsed((performance.now() - start) / 1000), 200);
    return () => clearInterval(id);
  }, [running && "on"]);

  /** One progress line of the test: a phase starts, a rate, or the result of a phase. */
  const apply = (line: string) => {
    if (line.startsWith("@")) {
      const [key, ...rest] = line.slice(1).split(" ");
      try {
        const data = JSON.parse(rest.join(" "));
        setPartial((p) => ({ ...p, [key]: data }));
      } catch {
        // a line this panel does not know: ignored
      }
      return;
    }
    if (line.startsWith("latency")) setPhase("ping");
    else if (line.startsWith("download")) {
      setPhase("down");
      setLive(0);
      setSamples([]);
      setBest(0);
    } else if (line.startsWith("upload")) {
      setPhase("up");
      setLive(0);
      setSamples([]);
      setBest(0);
    } else if (line.startsWith("UDP")) setPhase("udp");
    else {
      const m = /^[↓↑] (\d+(?:\.\d+)?) Mbit\/s/.exec(line);
      if (m) {
        const v = +m[1];
        setLive(v);
        setBest((b) => Math.max(b, v));
        setSamples((s) => [...s.slice(-59), v]);
      }
    }
  };

  const finish = (r: SpeedResult) => {
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
  };

  /** An agent that cannot be followed: one request, and the numbers come at the end. */
  const goBlocking = async () => {
    setPhase("ping");
    const r = await api.speedtest(tunnel.name, seconds, streams, udp);
    if (alive.current) finish(r);
  };

  /** The test is stopped: back to ready, with a note. */
  const wasStopped = () => {
    job.current = null;
    setStopping(false);
    setStopped(true);
    setPartial({});
    setLive(0);
    setSamples([]);
    setPhase("ready");
  };

  const stop = async () => {
    if (!job.current || stopping) return;
    setStopping(true);
    try {
      await api.speedtestStop(tunnel.name, job.current);
    } catch {
      // It may have just ended: the poll tells.
      setStopping(false);
    }
  };

  const go = async () => {
    setError("");
    setStopped(false);
    setStopping(false);
    job.current = null;
    setResult(null);
    setElapsed(0);
    setLive(0);
    setPartial({});
    setSamples([]);
    setBest(0);
    setPhase("ping");
    try {
      const started = await api.speedtestStart(tunnel.name, seconds, streams, udp);
      if (!alive.current) return;
      if (!started.ok) {
        if (/unknown request/i.test(started.error ?? "")) return await goBlocking();
        setError(started.error === "busy" ? t("sp.busy") : started.error ?? t("speed.failed"));
        setPhase("failed");
        return;
      }
      job.current = started.id;
      let after = 0;
      for (let misses = 0; ; ) {
        let p;
        try {
          p = await api.speedtestPoll(tunnel.name, started.id, after);
          misses = 0;
        } catch (e) {
          // A poll that fails now and then is not the end of the test.
          if (++misses > 5) throw e;
          await new Promise((r) => setTimeout(r, 600));
          continue;
        }
        if (!alive.current) return;
        after = p.next;
        p.lines.forEach(apply);
        if (!p.ok) {
          setError(p.error ?? t("speed.failed"));
          setPhase("failed");
          return;
        }
        if (p.done) {
          job.current = null;
          setStopping(false);
          if (p.error === "stopped") return wasStopped();
          finish({ ok: !p.error, error: p.error, text: "", report: p.report });
          return;
        }
        await new Promise((r) => setTimeout(r, 400));
      }
    } catch (e) {
      if (!alive.current) return;
      setError(e instanceof ApiError ? e.code : String(e));
      setPhase("failed");
    }
  };

  const report = result?.report ?? null;
  // What the gauge shows: the report, else the rate right now in the phase that runs, else what the finished phases measured.
  const down = report?.download.mbps ?? (phase === "down" ? live : partial.download?.mbps ?? 0);
  const up = report?.upload.mbps ?? (phase === "up" ? live : partial.upload?.mbps ?? 0);
  const scale = scaleFor(Math.max(down, up, best, report ? 0 : Math.max(tunnel.rate, 50)));
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
    const idle = report?.idle ?? partial.idle;
    const dl = report?.download ?? partial.download;
    const ul = report?.upload ?? partial.upload;
    const u = report?.udp ?? partial.udp;
    if (s === "ping") return idle ? `${num(idle.p50_ms, 0)} ms` : "";
    if (s === "down") return dl ? `${fmt(dl.mbps)} Mbps` : stepState(s) === "active" && live > 0 ? `${fmt(live)} Mbps` : "";
    if (s === "up") return ul ? `${fmt(ul.mbps)} Mbps` : stepState(s) === "active" && live > 0 ? `${fmt(live)} Mbps` : "";
    return u ? `${num(lossOf(u), 1)}%` : report ? t("sp.noUdp") : "";
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
        {phase === "done" && report
          ? `↓ ${fmt(down)} Mbps, ↑ ${fmt(up)} Mbps`
          : running
            ? (phase === "down" || phase === "up") && live > 0
              ? `${t(`sp.phase.${phase}`)}: ${fmt(live)} Mbps · ${t("sp.best", { v: fmt(best) })}`
              : t(`sp.phase.${phase}`)
            : ""}
      </div>
      {running && (phase === "down" || phase === "up") && live > 0 && (
        <p className="sp-now num" aria-hidden="true">
          {phase === "down" ? "↓" : "↑"} {fmt(live)} Mbps · {t("sp.best", { v: fmt(best) })}
        </p>
      )}
      {running && samples.length > 1 && (
        <svg className="sp-spark" viewBox="0 0 120 24" preserveAspectRatio="none" aria-hidden="true">
          <polyline points={samples.map((v, i) => `${(i / Math.max(1, samples.length - 1)) * 120},${22 - (v / Math.max(1, best)) * 20}`).join(" ")} />
        </svg>
      )}

      {phase === "done" && report && (
        <div className="sp-results">
          <div className="sp-card down">
            <span className="label"><bdi dir="ltr">↓</bdi> {t("sp.phase.down")}</span>
            <b className="num">{fmt(report.download.mbps)}</b>
            <small>
              <bdi dir="ltr">Mbps</bdi> · {t("sp.peak", { v: fmt(report.download.peak_mbps) })}
            </small>
          </div>
          <div className="sp-card up">
            <span className="label"><bdi dir="ltr">↑</bdi> {t("sp.phase.up")}</span>
            <b className="num">{fmt(report.upload.mbps)}</b>
            <small>
              <bdi dir="ltr">Mbps</bdi> · {t("sp.peak", { v: fmt(report.upload.peak_mbps) })}
            </small>
          </div>
          <div className="sp-card">
            <span className="label">{t("sp.ping")}</span>
            <b className="num">{num(report.idle.p50_ms, 0)}</b>
            <small>
              <bdi dir="ltr">ms · p99 {num(report.idle.p99_ms, 0)}</bdi>
            </small>
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
          <span className="label">{t("sp.preset")}</span>
          <Seg
            value={presetOf(seconds, streams)}
            options={(["quick", "standard", "max", "custom"] as Preset[]).map((p) => [p, t(`sp.preset.${p}`)] as [Preset, string])}
            onChange={(p) => {
              if (running) return;
              if (p === "custom") setCustom(true);
              else {
                setCustom(false);
                setSeconds(PRESETS[p].seconds);
                setStreams(PRESETS[p].streams);
              }
            }}
          />
        </div>
        {(custom || presetOf(seconds, streams) === "custom") && (
          <>
            <div className="sp-opt">
              <span className="label">{t("sp.length")}</span>
              <Seg value={String(seconds)} options={["5", "10", "20", "30", "60"].map((v) => [v, `${num(+v)}s`] as [string, string])} onChange={(v) => !running && setSeconds(+v)} />
            </div>
            <div className="sp-opt">
              <span className="label">{t("sp.streams")}</span>
              <Seg value={String(streams)} options={["1", "4", "8", "16", "32"].map((v) => [v, num(+v)] as [string, string])} onChange={(v) => !running && setStreams(+v)} />
            </div>
          </>
        )}
        <label className="check sp-opt">
          <input type="checkbox" checked={udp} disabled={running} onChange={(e) => setUdp(e.target.checked)} /> {t("sp.udp")}
        </label>
        <span className="grow" />
        {running && job.current ? (
          <button className="btn btn-danger solid sp-go" type="button" disabled={stopping} onClick={() => void stop()}>
            <Icon name="x" size={18} />
            {stopping ? t("sp.stopping") : t("sp.stop")}
          </button>
        ) : (
          <button className="btn btn-primary sp-go" type="button" disabled={!canRun || running} onClick={() => void go()}>
            <span className="shine" />
            <Icon name="bolt" size={18} />
            {phase === "done" || phase === "failed" ? t("sp.again") : t("sp.go")}
          </button>
        )}
      </div>
      {stopped && phase === "ready" && (
        <p className="muted small" role="status">
          {t("sp.stoppedNote")}
        </p>
      )}
      {running && !job.current && <p className="muted small">{t("sp.cannotStop")}</p>}
      {presetOf(seconds, streams) === "max" && <p className="muted small">{t("sp.maxHint")}</p>}
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
