import { useApp } from "./store";

type T = (k: string, v?: Record<string, string | number>) => string;

/** Whether `text` is empty (the panel's agents port) or a port whose next one exists too. */
export const agentPortOk = (text: string): boolean => {
  const p = text.trim();
  return p === "" || (/^\d{1,5}$/.test(p) && +p >= 1 && +p <= 65534);
};

/** The port an agent is to dial: the one typed, else the panel's agents port. */
export const agentPortOf = (text: string, main: number | null): number | null => (text.trim() ? +text.trim() : main);

/** The text of an error from giving a server an agents port of its own, or null for another error. */
export function agentPortError(t: T, code: string, names: Map<string, string>): string | null {
  const [what, who = ""] = code.split(":");
  if (what === "port_taken") {
    if (who === "panel") return t("add.port.takenPanel");
    if (who === "agents") return t("add.port.takenAgents");
    return t("add.port.takenBy", { name: names.get(who) ?? who });
  }
  if (what === "port_busy") return t("add.port.busy");
  if (what === "bad_port") return t("add.portBad");
  return null;
}

/**
 * The agents port a server's agent dials: empty for the panel's own (`main`), as every agent, or a
 * port of its own, which the panel then listens on (and on the next one) for that agent alone.
 */
export function AgentPortField({ id, value, onChange, main }: { id: string; value: string; onChange: (v: string) => void; main: number | null }) {
  const { t } = useApp();
  return (
    <div className="field">
      <label htmlFor={id}>{t("add.port")}</label>
      <input
        className="text mono"
        id={id}
        dir="ltr"
        inputMode="numeric"
        placeholder={main == null ? "" : String(main)}
        value={value}
        onChange={(e) => onChange(e.target.value.trim())}
        style={{ maxWidth: 160 }}
      />
      <span className="help">{t("add.portHelp", { main: main ?? "?" })}</span>
      {!agentPortOk(value) && <span className="err">{t("add.portBad")}</span>}
    </div>
  );
}
