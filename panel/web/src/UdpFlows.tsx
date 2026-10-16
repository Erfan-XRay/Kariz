import { useApp } from "./store";

const MAX = 65536;
const DEFAULT = 4096;

/** The UDP flow limit as the form holds it (text; empty or 0: the default), as what is sent. */
export function udpFlowsOf(text: string): { value?: number; bad?: boolean } {
  const s = text.trim();
  if (!s || s === "0") return {};
  if (!/^\d{1,5}$/.test(s) || +s > MAX) return { bad: true };
  return { value: +s };
}

/**
 * How many UDP clients each port of the tunnel takes at once (`tuning.udp_max_flows` on the entry side).
 * Packets from more are dropped while the ones open go on. Shown only for a tunnel that forwards UDP.
 */
export function UdpFlows({ id, value, onChange, protocol }: { id: string; value: string; onChange: (v: string) => void; protocol: string }) {
  const { t } = useApp();
  if (!protocol.includes("udp")) return null;
  const bad = !!udpFlowsOf(value).bad;
  return (
    <div className="field">
      <label htmlFor={id}>{t("udpflows.title")}</label>
      <input
        className={`text mono ${bad ? "is-bad" : ""}`}
        id={id}
        dir="ltr"
        inputMode="numeric"
        value={value}
        placeholder={t("udpflows.default", { n: DEFAULT })}
        aria-invalid={bad}
        onChange={(e) => onChange(e.target.value.trim())}
      />
      <span className={`help ${bad ? "err" : ""}`}>{t("udpflows.help", { n: DEFAULT, hi: MAX })}</span>
    </div>
  );
}
