import { useApp } from "./store";

const MAX = 100000;

/** The per-address limit as the form holds it (text; empty or 0: no limit), as what is sent. */
export function perIpOf(text: string): { value?: number; bad?: boolean } {
  const s = text.trim();
  if (!s || s === "0") return {};
  if (!/^\d{1,6}$/.test(s) || +s > MAX) return { bad: true };
  return { value: +s };
}

/**
 * How many connections one user address may hold open through the tunnel (`tuning.max_conns_per_ip` on the
 * entry side). Off unless filled in: on mobile networks many users share one address.
 */
export function PerIpLimit({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const { t } = useApp();
  const bad = !!perIpOf(value).bad;
  return (
    <div className="field">
      <label htmlFor={id}>{t("perip.title")}</label>
      <input
        className={`text mono ${bad ? "is-bad" : ""}`}
        id={id}
        dir="ltr"
        inputMode="numeric"
        value={value}
        placeholder={t("perip.none")}
        aria-invalid={bad}
        onChange={(e) => onChange(e.target.value.trim())}
      />
      <span className={`help ${bad ? "err" : ""}`}>{t("perip.help", { lo: 1, hi: MAX })}</span>
    </div>
  );
}
