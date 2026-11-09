import type { ReactNode } from "react";
import type { KcpSpec } from "./api";
import { useApp } from "./store";

/** The KCP presets the core knows, from gentle to aggressive. */
export type KcpMode = "normal" | "fast" | "fast2" | "fast3" | "manual";
export const KCP_MODES: KcpMode[] = ["normal", "fast", "fast2", "fast3", "manual"];

/** The Paqet KCP tuning as the form holds it: text, so a field can be empty (the profile's value). */
export interface KcpForm {
  mode: KcpMode | "";
  /** Manual timing: empty keeps fast2's value. */
  nodelay: "" | "on" | "off";
  interval_ms: string;
  resend: string;
  no_congestion: "" | "on" | "off";
  send_window: string;
  recv_window: string;
  mtu: string;
  fec: "default" | "off" | "custom";
  fec_data: string;
  fec_parity: string;
  datagrams: "" | "on" | "off";
}

/** The field of the KCP form that is wrong, or that a manual timing/MTU needs `manual`/less FEC. */
export type KcpBad =
  | "mode"
  | "timing"
  | "interval_ms"
  | "resend"
  | "send_window"
  | "recv_window"
  | "mtu"
  | "mtu_fec"
  | "fec_data"
  | "fec_parity";

type KcpField = "mode" | "interval_ms" | "resend" | "send_window" | "recv_window" | "mtu" | "fec_data" | "fec_parity";

export const emptyKcp = (): KcpForm => ({
  mode: "",
  nodelay: "",
  interval_ms: "",
  resend: "",
  no_congestion: "",
  send_window: "",
  recv_window: "",
  mtu: "",
  fec: "default",
  fec_data: "",
  fec_parity: "",
  datagrams: "",
});

const tri = (v?: boolean | null): "" | "on" | "off" => (v == null ? "" : v ? "on" : "off");
const text = (v?: number | null) => (v == null ? "" : String(v));

/** A `[tunnel.kcp]` table from a server, as the form holds it. */
export function kcpFromSpec(k?: KcpSpec | null): KcpForm {
  if (!k) return emptyKcp();
  const fec = k.fec_data == null && k.fec_parity == null ? "default" : (k.fec_data ?? 0) === 0 && (k.fec_parity ?? 0) === 0 ? "off" : "custom";
  return {
    mode: (k.mode ?? "") as KcpMode | "",
    nodelay: tri(k.nodelay),
    interval_ms: text(k.interval_ms),
    resend: text(k.resend),
    no_congestion: tri(k.no_congestion),
    send_window: text(k.send_window),
    recv_window: text(k.recv_window),
    mtu: text(k.mtu),
    fec,
    fec_data: text(k.fec_data),
    fec_parity: text(k.fec_parity),
    datagrams: tri(k.datagrams),
  };
}

/** A whole number in `lo..=hi`, `undefined` when the field is empty, `null` when it is not one. */
function numberIn(raw: string, lo: number, hi: number): number | null | undefined {
  const s = raw.trim();
  if (!s) return undefined;
  if (!/^\d{1,9}$/.test(s)) return null;
  const n = +s;
  return n >= lo && n <= hi ? n : null;
}

/** The form as a `[tunnel.kcp]` table (`undefined`: all defaults), or the first field that is wrong. */
export function kcpToSpec(f: KcpForm, profile: string): { spec?: KcpSpec; bad?: KcpBad } {
  if (f.mode && !KCP_MODES.includes(f.mode)) return { bad: "mode" };
  const spec: KcpSpec = {};
  if (f.mode) spec.mode = f.mode;

  const manual = f.mode === "manual";
  if (!manual && (f.nodelay || f.interval_ms.trim() || f.resend.trim() || f.no_congestion)) {
    return { bad: "timing" };
  }
  if (manual) {
    if (f.nodelay) spec.nodelay = f.nodelay === "on";
    const interval = numberIn(f.interval_ms, 10, 1000);
    if (interval === null) return { bad: "interval_ms" };
    if (interval !== undefined) spec.interval_ms = interval;
    const resend = numberIn(f.resend, 0, 10);
    if (resend === null) return { bad: "resend" };
    if (resend !== undefined) spec.resend = resend;
    if (f.no_congestion) spec.no_congestion = f.no_congestion === "on";
  }

  const send = numberIn(f.send_window, 16, 32768);
  if (send === null) return { bad: "send_window" };
  if (send !== undefined) spec.send_window = send;
  const recv = numberIn(f.recv_window, 128, 32768);
  if (recv === null) return { bad: "recv_window" };
  if (recv !== undefined) spec.recv_window = recv;

  const mtu = numberIn(f.mtu, 576, 1430);
  if (mtu === null) return { bad: "mtu" };
  if (mtu !== undefined) spec.mtu = mtu;

  if (f.fec === "off") {
    spec.fec_data = 0;
    spec.fec_parity = 0;
  } else if (f.fec === "custom") {
    const data = numberIn(f.fec_data, 1, 64);
    if (data === null || data === undefined) return { bad: "fec_data" };
    const parity = numberIn(f.fec_parity, 1, 32);
    if (parity === null || parity === undefined) return { bad: "fec_parity" };
    spec.fec_data = data;
    spec.fec_parity = parity;
  }

  const fec = f.fec === "custom" || (f.fec === "default" && profile === "gaming" && (mtu ?? 1350) <= 1429);
  if (fec && mtu !== undefined && mtu > 1417) return { bad: "mtu_fec" };

  if (f.datagrams) spec.datagrams = f.datagrams === "on";
  return { spec: Object.keys(spec).length ? spec : undefined };
}

/** The KCP tuning to send: only a Paqet tunnel has one, and an empty form sends nothing. */
export function kcpSpecOf(f: KcpForm, transport: string, profile: string): KcpSpec | undefined {
  return transport === "paqet" ? kcpToSpec(f, profile).spec : undefined;
}

/** What the core uses when a field is empty, as placeholders. */
function defaults(profile: string) {
  return {
    mode: "fast2",
    send_window: 1024,
    recv_window: 1024,
    mtu: 1350,
    fec_data: profile === "gaming" ? 10 : 0,
    fec_parity: profile === "gaming" ? 3 : 0,
  };
}

/** The KCP wire's own tuning for a Paqet tunnel; only that transport shows it. */
export function KcpFields({ value, onChange, profile }: { value: KcpForm; onChange: (v: KcpForm) => void; profile: string }) {
  const { t } = useApp();
  const d = defaults(profile);
  const bad = kcpToSpec(value, profile).bad;
  const changed = JSON.stringify(value) !== JSON.stringify(emptyKcp());
  const field = (key: KcpField, help: string, input: ReactNode, wrong = bad === key) => (
    <div className="field">
      <label htmlFor={`kcp-${key}`}>{t(`kcp.${key}`)}</label>
      {input}
      <span className={`help ${wrong ? "err" : ""}`} id={`kcp-${key}-help`}>
        {wrong ? t(`kcp.bad.${bad}`) : help}
      </span>
    </div>
  );
  return (
    <div className="mux-fields">
      <p className="muted small">{t("kcp.lead")}</p>
      <div className="grid-3">
        <div className="field">
          <label htmlFor="kcp-mode">{t("kcp.mode")}</label>
          <select
            className={`select ${bad === "mode" || bad === "timing" ? "is-bad" : ""}`}
            id="kcp-mode"
            value={value.mode}
            aria-invalid={bad === "mode" || bad === "timing"}
            aria-describedby="kcp-mode-help"
            onChange={(e) => {
              const mode = e.target.value as KcpMode | "";
              // The manual timing belongs to this mode: do not leave it hidden behind another.
              onChange(mode === "manual" ? { ...value, mode } : { ...value, mode, nodelay: "", interval_ms: "", resend: "", no_congestion: "" });
            }}
          >
            <option value="">{t("kcp.mode.default", { mode: d.mode })}</option>
            {KCP_MODES.map((m) => (
              <option key={m} value={m}>
                {t(`kcp.mode.${m}`)}
              </option>
            ))}
          </select>
          <span className={`help ${bad === "mode" || bad === "timing" ? "err" : ""}`} id="kcp-mode-help">
            {bad === "mode" ? t("kcp.bad.mode") : bad === "timing" ? t("kcp.bad.timing") : t(`kcp.mode.${value.mode || "default"}.d`, { mode: d.mode })}
          </span>
        </div>
        {value.mode === "manual" && (
          <>
            <div className="field">
              <label htmlFor="kcp-nodelay">{t("kcp.nodelay")}</label>
              <select className="select" id="kcp-nodelay" value={value.nodelay} onChange={(e) => onChange({ ...value, nodelay: e.target.value as KcpForm["nodelay"] })}>
                <option value="">{t("kcp.default")}</option>
                <option value="on">{t("kcp.on")}</option>
                <option value="off">{t("kcp.off")}</option>
              </select>
              <span className="help">{t("kcp.nodelay.d")}</span>
            </div>
            {field(
              "interval_ms",
              t("kcp.interval_ms.d"),
              <input
                className={`text mono ${bad === "interval_ms" ? "is-bad" : ""}`}
                id="kcp-interval_ms"
                dir="ltr"
                inputMode="numeric"
                value={value.interval_ms}
                placeholder="20"
                aria-invalid={bad === "interval_ms"}
                aria-describedby="kcp-interval_ms-help"
                onChange={(e) => onChange({ ...value, interval_ms: e.target.value.trim() })}
              />,
            )}
            {field(
              "resend",
              t("kcp.resend.d"),
              <input
                className={`text mono ${bad === "resend" ? "is-bad" : ""}`}
                id="kcp-resend"
                dir="ltr"
                inputMode="numeric"
                value={value.resend}
                placeholder="2"
                aria-invalid={bad === "resend"}
                aria-describedby="kcp-resend-help"
                onChange={(e) => onChange({ ...value, resend: e.target.value.trim() })}
              />,
            )}
            <div className="field">
              <label htmlFor="kcp-no_congestion">{t("kcp.no_congestion")}</label>
              <select
                className="select"
                id="kcp-no_congestion"
                value={value.no_congestion}
                onChange={(e) => onChange({ ...value, no_congestion: e.target.value as KcpForm["no_congestion"] })}
              >
                <option value="">{t("kcp.default")}</option>
                <option value="on">{t("kcp.on")}</option>
                <option value="off">{t("kcp.off")}</option>
              </select>
              <span className="help">{t("kcp.no_congestion.d")}</span>
            </div>
          </>
        )}
        {field(
          "send_window",
          t("kcp.send_window.d"),
          <input
            className={`text mono ${bad === "send_window" ? "is-bad" : ""}`}
            id="kcp-send_window"
            dir="ltr"
            inputMode="numeric"
            value={value.send_window}
            placeholder={String(d.send_window)}
            aria-invalid={bad === "send_window"}
            aria-describedby="kcp-send_window-help"
            onChange={(e) => onChange({ ...value, send_window: e.target.value.trim() })}
          />,
        )}
        {field(
          "recv_window",
          t("kcp.recv_window.d"),
          <input
            className={`text mono ${bad === "recv_window" ? "is-bad" : ""}`}
            id="kcp-recv_window"
            dir="ltr"
            inputMode="numeric"
            value={value.recv_window}
            placeholder={String(d.recv_window)}
            aria-invalid={bad === "recv_window"}
            aria-describedby="kcp-recv_window-help"
            onChange={(e) => onChange({ ...value, recv_window: e.target.value.trim() })}
          />,
        )}
        {field(
          "mtu",
          t("kcp.mtu.d"),
          <input
            className={`text mono ${bad === "mtu" || bad === "mtu_fec" ? "is-bad" : ""}`}
            id="kcp-mtu"
            dir="ltr"
            inputMode="numeric"
            value={value.mtu}
            placeholder={String(d.mtu)}
            aria-invalid={bad === "mtu" || bad === "mtu_fec"}
            aria-describedby="kcp-mtu-help"
            onChange={(e) => onChange({ ...value, mtu: e.target.value.trim() })}
          />,
          bad === "mtu" || bad === "mtu_fec",
        )}
        <div className="field">
          <label htmlFor="kcp-fec">{t("kcp.fec")}</label>
          <select
            className="select"
            id="kcp-fec"
            value={value.fec}
            aria-invalid={bad === "fec_data" || bad === "fec_parity"}
            aria-describedby="kcp-fec-help"
            onChange={(e) => onChange({ ...value, fec: e.target.value as KcpForm["fec"] })}
          >
            <option value="default">{t("kcp.fec.default")}</option>
            <option value="off">{t("kcp.fec.off")}</option>
            <option value="custom">{t("kcp.fec.custom")}</option>
          </select>
          <span className={`help ${bad === "fec_data" || bad === "fec_parity" ? "err" : ""}`} id="kcp-fec-help">
            {bad === "fec_data" || bad === "fec_parity" ? t(`kcp.bad.${bad}`) : t("kcp.fec.d")}
          </span>
        </div>
        {value.fec === "custom" && (
          <>
            {field(
              "fec_data",
              t("kcp.fec_data.d"),
              <input
                className={`text mono ${bad === "fec_data" ? "is-bad" : ""}`}
                id="kcp-fec_data"
                dir="ltr"
                inputMode="numeric"
                value={value.fec_data}
                placeholder={String(d.fec_data)}
                aria-invalid={bad === "fec_data"}
                aria-describedby="kcp-fec_data-help"
                onChange={(e) => onChange({ ...value, fec_data: e.target.value.trim() })}
              />,
            )}
            {field(
              "fec_parity",
              t("kcp.fec_parity.d"),
              <input
                className={`text mono ${bad === "fec_parity" ? "is-bad" : ""}`}
                id="kcp-fec_parity"
                dir="ltr"
                inputMode="numeric"
                value={value.fec_parity}
                placeholder={String(d.fec_parity)}
                aria-invalid={bad === "fec_parity"}
                aria-describedby="kcp-fec_parity-help"
                onChange={(e) => onChange({ ...value, fec_parity: e.target.value.trim() })}
              />,
            )}
          </>
        )}
        <div className="field">
          <label htmlFor="kcp-datagrams">{t("kcp.datagrams")}</label>
          <select className="select" id="kcp-datagrams" value={value.datagrams} onChange={(e) => onChange({ ...value, datagrams: e.target.value as KcpForm["datagrams"] })}>
            <option value="">{t("kcp.default")}</option>
            <option value="on">{t("kcp.on")}</option>
            <option value="off">{t("kcp.off")}</option>
          </select>
          <span className="help">{t("kcp.datagrams.d")}</span>
        </div>
      </div>
      <button className="btn btn-quiet btn-sm" type="button" disabled={!changed} onClick={() => onChange(emptyKcp())}>
        {t("kcp.reset")}
      </button>
    </div>
  );
}
