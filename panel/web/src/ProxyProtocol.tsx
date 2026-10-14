import { useApp } from "./store";
import { Icon, Seg } from "./ui";

export type ProxyChoice = "off" | "v1" | "v2";

/** The PROXY header choice as what is sent: nothing when it is off, or when no port carries TCP. */
export const proxyOf = (value: ProxyChoice, protocol: string): string | undefined =>
  value === "off" || protocol === "udp" ? undefined : value;

/** A tunnel's choice as read back from its entry side. */
export const proxyFromSpec = (value?: string): ProxyChoice => (value === "v1" || value === "v2" ? value : "off");

/**
 * Whether the target behind the exit side (usually Xray) is told each user's address in a PROXY header
 * (`proxy_protocol` on every TCP rule of the entry side). Off unless chosen: a target that does not expect
 * the header breaks on it, so turning it on comes with what to set on Xray first.
 */
export function ProxyProtocol({ value, onChange, protocol }: { value: ProxyChoice; onChange: (v: ProxyChoice) => void; protocol: string }) {
  const { t } = useApp();
  const udpOnly = protocol === "udp";
  return (
    <div className="field">
      <span className="label">{t("proxy.title")}</span>
      <Seg
        value={value}
        options={[
          ["off", t("proxy.off")],
          ["v1", "v1"],
          ["v2", "v2"],
        ]}
        onChange={onChange}
      />
      <span className="help">{t(udpOnly ? "proxy.udp" : "proxy.help")}</span>
      {!udpOnly && value !== "off" && (
        <span className="help warn-text" role="status">
          <Icon name="alert" size={14} /> {t("proxy.warn")}
        </span>
      )}
    </div>
  );
}
