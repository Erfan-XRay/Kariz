import type { Network } from "./api";
import { describeError } from "./ops";
import { useApp } from "./store";

type T = (k: string, v?: Record<string, string | number>) => string;

/** Whether `ip` is an IPv4 address written out in full. */
export const ipv4Ok = (ip: string): boolean => /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.test(ip.trim()) && ip.trim().split(".").every((p) => +p <= 255);

/** The text of an error from making a GRE link for a server's link to the panel (*Add server* or *Edit*). */
export function greJoinError(t: T, code: string, names: Map<string, string>): string {
  const [what, who = ""] = code.split(":");
  if (what === "no_address") return t("add.gre.err.noPanelAddr");
  if (what === "same_address") return t("add.gre.err.same");
  if (what === "address_taken") return t("add.gre.err.taken", { name: names.get(who) ?? who });
  if (what === "bad_input") return t("add.gre.err.ip");
  if (what === "agents_off") return t("add.off");
  return describeError(t, code, names);
}

/**
 * A GRE link the panel is to make to a server: that server's public IPv4 address, the panel's end
 * (its server's public address, `undefined` while it loads, `null` when it is not known) and the
 * private network the link's addresses come from.
 */
export function NewGreLinkFields({
  id,
  ip,
  onIp,
  ipLabel,
  error,
  networks,
  netId,
  onNet,
  panelPublic,
}: {
  id: string;
  ip: string;
  onIp: (ip: string) => void;
  ipLabel: string;
  error: string;
  networks: Network[];
  netId: string;
  onNet: (id: string) => void;
  panelPublic: string | null | undefined;
}) {
  const { t } = useApp();
  const network = networks.find((n) => n.id === netId) ?? networks[0];
  return (
    <>
      <div className="field">
        <label htmlFor={id}>{ipLabel}</label>
        <input className="text mono" id={id} dir="ltr" inputMode="decimal" placeholder="198.51.100.7" value={ip} onChange={(e) => onIp(e.target.value.trim())} />
        <span className="help">{t("add.gre.ipHelp")}</span>
        <span className="err" role="alert">
          {error}
        </span>
      </div>
      <div className="field">
        <span className="label">{t("add.gre.panel")}</span>
        {panelPublic ? (
          <code dir="ltr" className="gre-panel-addr">
            {panelPublic}
          </code>
        ) : panelPublic === null ? (
          <span className="err small">{t("add.gre.err.noPanelAddr")}</span>
        ) : (
          <span className="muted small">…</span>
        )}
        <span className="help">{t("add.gre.panelHelp")}</span>
      </div>
      <div className="field">
        <span className="label">{t("add.gre.net")}</span>
        {networks.length > 1 ? (
          <select className="select" aria-label={t("add.gre.net")} value={network?.id ?? ""} onChange={(e) => onNet(e.target.value)} style={{ maxWidth: 360 }}>
            {networks.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name} ({n.cidr})
              </option>
            ))}
          </select>
        ) : network ? (
          <span className="help">
            {t("add.gre.netOne", { name: network.name })} <code dir="ltr">{network.cidr}</code>
          </span>
        ) : (
          <span className="help">{t("add.gre.netNew")}</span>
        )}
      </div>
    </>
  );
}
