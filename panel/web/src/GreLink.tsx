import { kindLabel } from "./api";
import type { LinkKind, Network } from "./api";
import { describeError } from "./ops";
import { useApp } from "./store";
import { Seg } from "./ui";

type T = (k: string, v?: Record<string, string | number>) => string;

/** Whether `ip` is an IPv4 address written out in full. */
export const ipv4Ok = (ip: string): boolean => /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.test(ip.trim()) && ip.trim().split(".").every((p) => +p <= 255);

/** Whether `ip` is an IPv6 address a server can be reached at (not link-local, loopback, multicast or IPv4-mapped). */
export function ipv6Ok(ip: string): boolean {
  const a = ip.trim().toLowerCase();
  if (!a.includes(":") || !/^[0-9a-f:.]+$/.test(a)) return false;
  try {
    const host = new URL(`http://[${a}]/`).hostname;
    return !(/^\[fe[89ab]/.test(host) || host === "[::1]" || host === "[::]" || host.startsWith("[ff") || host.startsWith("[::ffff:"));
  } catch {
    return false;
  }
}

/** Whether `ip` can be the far end of a link: an IPv4 address, or an IPv6 one (a link over IPv6). */
export const linkIpOk = (ip: string): boolean => ipv4Ok(ip) || ipv6Ok(ip);

/** The panel server's end of a link to `ip`: its IPv6 address for an IPv6 one, else its IPv4 one (`undefined` while it loads, `null` when it is not known). */
export function panelEnd(own: { gre_local?: string | null; gre_local6?: string | null } | null, ip: string): string | null | undefined {
  if (!own) return undefined;
  return (ipv6Ok(ip) ? own.gre_local6 : own.gre_local) ?? null;
}

/** The text of an error from making a GRE link for a server's link to the panel (*Add server* or *Edit*). */
export function greJoinError(t: T, code: string, names: Map<string, string>): string {
  const [what, who = ""] = code.split(":");
  if (what === "no_address") return t("add.gre.err.noPanelAddr");
  if (what === "no_address6") return t("add.gre.err.noPanelAddr6");
  if (what === "same_address") return t("add.gre.err.same");
  if (what === "address_taken") return t("add.gre.err.taken", { name: names.get(who) ?? who });
  if (what === "bad_input") return t("add.gre.err.ip");
  if (what === "agents_off") return t("add.off");
  return describeError(t, code, names);
}

/** The network of `kind` a new link takes its addresses from: the one picked, if it is of that kind (none: the first, or a new one). */
export const networkOfKind = (networks: Network[], picked: string, kind: LinkKind): string | undefined =>
  networks.find((n) => n.id === picked && n.kind === kind)?.id;

/**
 * A GRE or VXLAN link the panel is to make to a server: its kind (with `onKind`, a choice between the
 * two), that server's public IPv4 address, the panel's end (its server's public address, `undefined`
 * while it loads, `null` when it is not known) and the private network of that kind the link's
 * addresses come from.
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
  kind,
  onKind,
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
  kind: LinkKind;
  onKind?: (kind: LinkKind) => void;
}) {
  const { t } = useApp();
  const pool = networks.filter((n) => (n.kind ?? "gre") === kind);
  const network = pool.find((n) => n.id === netId) ?? pool[0];
  return (
    <>
      {onKind && (
        <div className="field">
          <span className="label">{t("net.kind")}</span>
          <Seg<LinkKind>
            value={kind}
            options={[
              ["gre", "GRE"],
              ["vxlan", "VXLAN"],
            ]}
            onChange={onKind}
          />
          <span className="help">{t(kind === "vxlan" ? "net.kind.vxlan" : "net.kind.gre")}</span>
        </div>
      )}
      <div className="field">
        <label htmlFor={id}>{ipLabel}</label>
        <input className="text mono" id={id} dir="ltr" inputMode="text" placeholder="198.51.100.7" value={ip} onChange={(e) => onIp(e.target.value.trim())} />
        <span className="help">{t("add.gre.ipHelp", { kind: kindLabel(kind) })}</span>
        {ipv6Ok(ip) && (
          <span className="help">
            <span className="tag">IPv6</span> {t("add.gre.over6")}
          </span>
        )}
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
          <span className="err small">{t(ipv6Ok(ip) ? "add.gre.err.noPanelAddr6" : "add.gre.err.noPanelAddr")}</span>
        ) : (
          <span className="muted small">…</span>
        )}
        <span className="help">{t("add.gre.panelHelp")}</span>
      </div>
      <div className="field">
        <span className="label">{t("add.gre.net")}</span>
        {pool.length > 1 ? (
          <select className="select" aria-label={t("add.gre.net")} value={network?.id ?? ""} onChange={(e) => onNet(e.target.value)} style={{ maxWidth: 360 }}>
            {pool.map((n) => (
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
          <span className="help">{t(kind === "vxlan" ? "add.vx.netNew" : "add.gre.netNew")}</span>
        )}
      </div>
    </>
  );
}
