import type { MuxSpec } from "./api";
import { useApp } from "./store";

/** The mux settings as the form holds them: text, so a field can be empty (the profile's value). */
export interface MuxForm {
  connections: string;
  max_streams: string;
  /** In KiB. */
  stream_window: string;
  max_lifetime_secs: string;
  ping_interval_secs: string;
  coalesce: "default" | "on" | "off";
}

export const emptyMux = (): MuxForm => ({ connections: "", max_streams: "", stream_window: "", max_lifetime_secs: "", ping_interval_secs: "", coalesce: "default" });

export function muxFromSpec(m?: MuxSpec | null): MuxForm {
  if (!m) return emptyMux();
  const s = (v?: number | null) => (v == null ? "" : String(v));
  return {
    connections: s(m.connections),
    max_streams: s(m.max_streams),
    stream_window: m.stream_window == null ? "" : String(Math.round(m.stream_window / 1024)),
    max_lifetime_secs: s(m.max_lifetime_secs),
    ping_interval_secs: s(m.ping_interval_secs),
    coalesce: m.coalesce == null ? "default" : m.coalesce ? "on" : "off",
  };
}

const RANGES: Record<Exclude<keyof MuxForm, "coalesce">, [number, number]> = {
  connections: [1, 64],
  max_streams: [1, 4096],
  stream_window: [16, 16384],
  max_lifetime_secs: [60, 86400 * 30],
  ping_interval_secs: [1, 600],
};

/** The form as a spec (`undefined`: all defaults), or the name of the first field that is wrong. */
export function muxToSpec(f: MuxForm): { spec?: MuxSpec; bad?: keyof MuxForm } {
  const spec: MuxSpec = {};
  for (const key of Object.keys(RANGES) as (keyof typeof RANGES)[]) {
    const text = f[key].trim();
    if (!text) continue;
    const [lo, hi] = RANGES[key];
    if (!/^\d{1,9}$/.test(text) || +text < lo || +text > hi) return { bad: key };
    spec[key] = key === "stream_window" ? +text * 1024 : +text;
  }
  if (f.coalesce !== "default") spec.coalesce = f.coalesce === "on";
  return { spec: Object.keys(spec).length ? spec : undefined };
}

/** What the profile uses when a field is empty, as placeholders. */
function defaults(profile: string, transport: string) {
  const p = profile === "ultraspeed" || profile === "throughput" ? 1 : profile === "gaming" ? 2 : 0;
  return {
    connections: [4, 8, 2][p],
    max_streams: 512,
    stream_window: [256, 1024, 64][p],
    ping_interval_secs: transport === "auto" ? 2 : p === 2 ? 10 : 30,
    coalesce: p !== 2,
  };
}

/** The mux settings of a tunnel, all optional. */
export function MuxFields({ value, onChange, profile, transport }: { value: MuxForm; onChange: (v: MuxForm) => void; profile: string; transport: string }) {
  const { t } = useApp();
  const d = defaults(profile, transport);
  const bad = muxToSpec(value).bad;
  const num = (key: Exclude<keyof MuxForm, "coalesce">, placeholder: string | number) => {
    const [lo, hi] = RANGES[key];
    return (
      <div className="field">
        <label htmlFor={`mux-${key}`}>{t(`mux.${key}`)}</label>
        <input
          className={`text mono ${bad === key ? "is-bad" : ""}`}
          id={`mux-${key}`}
          dir="ltr"
          inputMode="numeric"
          value={value[key]}
          placeholder={String(placeholder)}
          aria-invalid={bad === key}
          onChange={(e) => onChange({ ...value, [key]: e.target.value.trim() })}
        />
        <span className={`help ${bad === key ? "err" : ""}`}>{t(`mux.${key}.d`, { lo, hi })}</span>
      </div>
    );
  };
  return (
    <div className="mux-fields">
      <p className="muted small">{t("mux.lead")}</p>
      <div className="grid-3">
        {num("connections", d.connections)}
        {num("max_streams", d.max_streams)}
        {num("stream_window", d.stream_window)}
        {num("ping_interval_secs", d.ping_interval_secs)}
        {num("max_lifetime_secs", t("mux.never"))}
        <div className="field">
          <label htmlFor="mux-coalesce">{t("mux.coalesce")}</label>
          <select className="select" id="mux-coalesce" value={value.coalesce} onChange={(e) => onChange({ ...value, coalesce: e.target.value as MuxForm["coalesce"] })}>
            <option value="default">{t(d.coalesce ? "mux.default.on" : "mux.default.off")}</option>
            <option value="on">{t("mux.on")}</option>
            <option value="off">{t("mux.off")}</option>
          </select>
          <span className="help">{t("mux.coalesce.d")}</span>
        </div>
      </div>
      <button className="btn btn-quiet btn-sm" type="button" onClick={() => onChange(emptyMux())}>
        {t("mux.reset")}
      </button>
    </div>
  );
}
