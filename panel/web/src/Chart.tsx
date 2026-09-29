import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import { useApp } from "./store";
import { Seg } from "./ui";

const RANGES = ["1h", "24h", "7d", "30d"] as const;
type Range = (typeof RANGES)[number];

/** A time series from the panel's history, as a line with the water under it. */
export function Chart({ series, unit, decimals = 1 }: { series: string; unit: string; decimals?: number }) {
  const { t, num } = useApp();
  const [range, setRange] = useState<Range>("1h");
  const [points, setPoints] = useState<[number, number][]>([]);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    let alive = true;
    const load = () =>
      api
        .history(series, range)
        .then((r) => alive && setPoints(r.points))
        .catch(() => {});
    void load();
    const id = setInterval(load, range === "1h" ? 5000 : 30000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [series, range]);

  const W = 600;
  const H = 150;
  const { path, area, max, coords } = useMemo(() => {
    if (points.length < 2) return { path: "", area: "", max: 0, coords: [] as [number, number][] };
    const t0 = points[0][0];
    const t1 = points[points.length - 1][0];
    const top = Math.max(...points.map((p) => p[1]), 0.0001) * 1.15;
    const cs = points.map(([ts, v]) => [((ts - t0) / Math.max(1, t1 - t0)) * W, H - (v / top) * (H - 8) - 2] as [number, number]);
    const line = cs.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" ");
    return { path: line, area: `${line} L${W} ${H} L0 ${H} Z`, max: top / 1.15, coords: cs };
  }, [points]);

  const at = hover !== null && points[hover] ? points[hover] : null;
  const time = (ts: number) => new Date(ts * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const kind = series.split(":")[2];

  return (
    <div className="chart-block">
      <div className="chart-head">
        <span className="label">{t(`chart.${kind}`)}</span>
        <Seg value={range} options={RANGES.map((r) => [r, r] as [Range, string])} onChange={setRange} />
      </div>
      <div className="chart-wrap">
        {points.length < 2 ? (
          <p className="muted small" style={{ padding: "var(--sp-5) 0" }}>
            {t("chart.none")}
          </p>
        ) : (
          <svg
            className="chart"
            viewBox={`0 0 ${W} ${H}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={t(`chart.${kind}`)}
            onMouseLeave={() => setHover(null)}
            onMouseMove={(e) => {
              const box = e.currentTarget.getBoundingClientRect();
              const rtl = document.documentElement.dir === "rtl";
              const f = (e.clientX - box.left) / box.width;
              const x = (rtl ? 1 - f : f) * W;
              let best = 0;
              coords.forEach(([cx], i) => {
                if (Math.abs(cx - x) < Math.abs(coords[best][0] - x)) best = i;
              });
              setHover(best);
            }}
          >
            <defs>
              <linearGradient id={`g-${series}`} x1="0" x2="0" y1="0" y2="1">
                <stop offset="0" stopColor="var(--water)" stopOpacity="0.35" />
                <stop offset="1" stopColor="var(--water)" stopOpacity="0.02" />
              </linearGradient>
            </defs>
            <path d={area} fill={`url(#g-${series})`} />
            <path d={path} fill="none" stroke="var(--water)" strokeWidth="2" vectorEffect="non-scaling-stroke" />
            {hover !== null && coords[hover] && <circle cx={coords[hover][0]} cy={coords[hover][1]} r="4" fill="var(--accent)" />}
          </svg>
        )}
        {at && (
          <div className="chart-tip is-on" style={{ insetInlineStart: 8 }}>
            {num(at[1], decimals)} {unit} · {time(at[0])}
          </div>
        )}
      </div>
      {points.length >= 2 && (
        <div className="legend">
          <span>
            {t("chart.max")} {num(max, decimals)} {unit}
          </span>
        </div>
      )}
    </div>
  );
}
