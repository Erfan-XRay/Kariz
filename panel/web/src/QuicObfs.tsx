import { useApp } from "./store";
import { Icon } from "./ui";

/** The sealed-QUIC switch of a `quic` tunnel, and what it sends: nothing for another transport. */
export const obfsOf = (value: boolean, transport: string): boolean | undefined => (transport === "quic" && value ? true : undefined);

/**
 * Seal every UDP packet of a QUIC tunnel with a key from the token (`[tunnel.quic] obfs`), so a
 * network that filters QUIC does not recognise it. On by default in the wizard: it is what makes
 * QUIC usable where it is filtered, and it costs 28 bytes a packet and a little CPU.
 */
export function QuicObfs({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  const { t } = useApp();
  return (
    <div className="field quic-obfs">
      <span className="label">{t("obfs.title")}</span>
      <label className="check">
        <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} /> {t("obfs.label")}
      </label>
      <span className="help">{t("obfs.help")}</span>
      {!value && (
        <span className="help warn-text" role="status">
          <Icon name="alert" size={14} /> {t("obfs.off")}
        </span>
      )}
    </div>
  );
}
