import type { Link, Network } from "./api";
import { useApp } from "./store";

/** The link between two servers in a network, if there is one. */
export const linkBetween = (links: Link[], network: string, a: string, b: string): Link | undefined =>
  links.find((l) => l.network === network && [l.a, l.b].sort().join() === [a, b].sort().join());

/** The link that has `host` as the address of one of its ends: what a tunnel over a private network listens on. */
export const linkWithAddress = (links: Link[], host: string): Link | undefined => (host ? links.find((l) => l.addr_a === host || l.addr_b === host) : undefined);

/**
 * Run the tunnel over a private GRE network: it listens on and dials the private address of the
 * server that accepts its connections, in either mode, so it never uses the public ones. Used by the
 * wizard and by the edit page (where unticking it puts the tunnel back on the public addresses).
 */
export function GreChoice({
  value,
  onChange,
  networks,
  links,
  entry,
  exit,
  acceptor,
  port,
  serverName,
}: {
  value: string;
  onChange: (network: string) => void;
  networks: Network[];
  links: Link[];
  entry: string;
  exit: string;
  /** The server that listens for the tunnel's connections (the entry when reverse, the exit when direct). */
  acceptor: string;
  port: string;
  serverName: (id: string) => string;
}) {
  const { t } = useApp();
  const link = value ? linkBetween(links, value, entry, exit) : undefined;
  const addr = link ? (link.a === acceptor ? link.addr_a : link.addr_b) : "";
  return (
    <div className="field gre-choice" style={{ marginBottom: "var(--sp-5)" }}>
      <label className="check">
        <input type="checkbox" checked={!!value} disabled={networks.length === 0 && !value} onChange={(e) => onChange(e.target.checked ? networks[0]?.id ?? "" : "")} /> {t("wz.gre")}
      </label>
      {networks.length === 0 && !value && <span className="help">{t("wz.greNone")}</span>}
      {value && (
        <>
          <select className="select" aria-label={t("wz.greNet")} value={value} onChange={(e) => onChange(e.target.value)} style={{ maxWidth: 360 }}>
            {networks.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name} ({n.cidr})
              </option>
            ))}
          </select>
          <span className="help">{t("wz.greText")}</span>
          {/* The address is isolated left to right, so a Persian sentence around it keeps its order. */}
          <span className="help">{link ? t("wz.greAddrs", { addr: `⁦${addr}:${port}⁩`, server: `⁨${serverName(acceptor)}⁩` }) : t("wz.greNew")}</span>
        </>
      )}
    </div>
  );
}
