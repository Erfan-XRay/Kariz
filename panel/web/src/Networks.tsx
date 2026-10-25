import { useEffect, useState } from "react";
import { ApiError, api, kindLabel } from "./api";
import type { Link, LinkKind, Network, ServerInfo } from "./api";
import { Checklist, describeError, useOp } from "./ops";
import { useApp } from "./store";
import { Card, Dialog, Empty, Icon, Seg } from "./ui";

/** VXLAN's usual UDP port. */
const VXLAN_PORT = 4789;

/** A network's kind as a short tag: `GRE`, or `VXLAN · UDP 4789`. */
export function kindTag(n: { kind?: LinkKind; port?: number }): string {
  return n.kind === "vxlan" ? `VXLAN · UDP ${n.port || VXLAN_PORT}` : "GRE";
}

/** The networks and links of the panel, kept fresh while the page is open. */
export function useNetworks(): { networks: Network[]; links: Link[]; loaded: boolean; reload: () => void } {
  const [data, setData] = useState<{ networks: Network[]; links: Link[]; loaded: boolean }>({ networks: [], links: [], loaded: false });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    const load = () =>
      api
        .networks()
        .then((r) => alive && setData({ ...r, loaded: true }))
        .catch(() => {});
    void load();
    const id = setInterval(load, 3000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [tick]);
  return { ...data, reload: () => setTick((n) => n + 1) };
}

/** Ready-made address pools, from large to small. */
const POOLS = ["10.77.0.0/16", "10.88.0.0/16", "10.200.0.0/16", "172.30.0.0/16", "192.168.222.0/24"] as const;

/** `a.b.c.d/p` as its first and last address, or null. */
function range(cidr: string): [number, number] | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/.exec(cidr.trim());
  if (!m) return null;
  const o = m.slice(1, 5).map(Number);
  const p = Number(m[5]);
  if (o.some((x) => x > 255) || p > 32) return null;
  const ip = ((o[0] * 256 + o[1]) * 256 + o[2]) * 256 + o[3];
  const size = 2 ** (32 - p);
  const first = Math.floor(ip / size) * size;
  return [first, first + size - 1];
}

const overlaps = (a: string, b: string) => {
  const x = range(a);
  const y = range(b);
  return !!x && !!y && x[0] <= y[1] && y[0] <= x[1];
};

function NewNetwork({ onClose, servers }: { onClose: () => void; servers: ServerInfo[] }) {
  const { t, num } = useApp();
  const [name, setName] = useState("main");
  // The first pool that no server already routes.
  const used = servers.flatMap((s) => s.health?.routes ?? []);
  const free = (c: string) => !used.some((r) => overlaps(c, r));
  const [cidr, setCidr] = useState<string>(POOLS.find(free) ?? POOLS[0]);
  const [kind, setKind] = useState<LinkKind>("gre");
  const [port, setPort] = useState(String(VXLAN_PORT));
  const [error, setError] = useState("");
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const portNum = Number(port.trim());
  const portOk = kind === "gre" || (/^\d{1,5}$/.test(port.trim()) && portNum >= 1 && portNum <= 65535);
  const make = async () => {
    setError("");
    try {
      await api.createNetwork(name.trim(), cidr.trim(), kind, kind === "vxlan" ? portNum : undefined);
      onClose();
    } catch (e) {
      setError(e instanceof ApiError ? describeError(t, e.code, names) : String(e));
    }
  };
  return (
    <Dialog
      title={t("net.new")}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={!portOk} onClick={() => void make()}>
            {t("net.make")}
          </button>
        </>
      }
    >
      <p className="muted small">{t("net.newText")}</p>
      <div className="field">
        <span className="label">{t("net.kind")}</span>
        <Seg<LinkKind>
          value={kind}
          options={[
            ["gre", "GRE"],
            ["vxlan", "VXLAN"],
          ]}
          onChange={setKind}
        />
        <span className="help">{t(kind === "vxlan" ? "net.kind.vxlan" : "net.kind.gre")}</span>
      </div>
      {kind === "vxlan" && (
        <div className="field">
          <label htmlFor="nn-port">{t("net.port")}</label>
          <input className="text mono" id="nn-port" dir="ltr" inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value)} />
          <span className="help">{t("net.portHelp")}</span>
          {!portOk && <span className="err">{t("net.portBad")}</span>}
        </div>
      )}
      <div className="field">
        <label htmlFor="nn-name">{t("wz.name")}</label>
        <input className="text mono" id="nn-name" dir="ltr" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label htmlFor="nn-cidr">{t("net.pool")}</label>
        <div className="pool-picks" role="group" aria-label={t("net.pools")}>
          {POOLS.map((c) => {
            const r = range(c)!;
            const links = Math.floor((r[1] - r[0] + 1) / 4);
            const taken = !free(c);
            return (
              <button key={c} type="button" className="addr-pick" aria-pressed={cidr.trim() === c} disabled={taken} onClick={() => setCidr(c)}>
                <code dir="ltr">{c}</code>
                <span>{taken ? t("net.poolUsed") : t("net.poolLinks", { n: num(links) })}</span>
              </button>
            );
          })}
        </div>
        <input className="text mono" id="nn-cidr" dir="ltr" value={cidr} onChange={(e) => setCidr(e.target.value)} />
        <span className="help">{t("net.poolHelp")}</span>
        <span className="err">{error}</span>
      </div>
    </Dialog>
  );
}

function AddLinks({ network, servers, onClose }: { network: Network; servers: ServerInfo[]; onClose: () => void }) {
  const { t } = useApp();
  const online = servers.filter((s) => s.online);
  const [picked, setPicked] = useState<string[]>(online.slice(0, 2).map((s) => s.id));
  const [hub, setHub] = useState("");
  const [id, setId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const op = useOp(id);
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const running = !!id && (!op || op.state === "running");
  const go = async () => {
    setError("");
    try {
      setId((await api.createLinks(network.id, picked, hub && picked.includes(hub) ? hub : undefined)).op);
    } catch (e) {
      setError(e instanceof ApiError ? describeError(t, e.code, names) : String(e));
    }
  };
  const count = hub && picked.includes(hub) ? picked.length - 1 : (picked.length * (picked.length - 1)) / 2;
  return (
    <Dialog
      title={t("net.addLinks", { name: network.name })}
      onClose={running ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={running} onClick={onClose}>
            {op?.state === "done" ? t("wz.close") : t("cancel")}
          </button>
          {!id && (
            <button className="btn btn-primary btn-sm" type="button" disabled={picked.length < 2} onClick={() => void go()}>
              {t("net.linksMake", { n: count })}
            </button>
          )}
          {op?.state === "failed" && (
            <button className="btn btn-primary btn-sm" type="button" onClick={() => setId(null)}>
              {t("wz.retry")}
            </button>
          )}
        </>
      }
    >
      {!id && (
        <>
          <p className="muted small">{t("net.linksText", { kind: kindLabel(network.kind) })}</p>
          <div className="pick-list" role="group" aria-label={t("m.servers")}>
            {servers.map((s) => (
              <label key={s.id} className="pick" aria-disabled={!s.online} style={{ gridTemplateColumns: "auto 1fr auto" }}>
                <input
                  type="checkbox"
                  disabled={!s.online}
                  checked={picked.includes(s.id)}
                  onChange={(e) => setPicked(e.target.checked ? [...picked, s.id] : picked.filter((x) => x !== s.id))}
                />
                <span>
                  <span className="nm">{s.name}</span>
                  <br />
                  <span className="mt mono" dir="ltr">
                    {s.addr ?? s.addr_default ?? t("net.noAddr")}
                  </span>
                </span>
                <span />
              </label>
            ))}
          </div>
          {picked.length > 2 && (
            <div className="field">
              <label htmlFor="al-hub">{t("net.hub")}</label>
              <select className="select" id="al-hub" value={hub} onChange={(e) => setHub(e.target.value)}>
                <option value="">{t("net.mesh")}</option>
                {picked.map((p) => (
                  <option key={p} value={p}>
                    {t("net.hubOf", { name: names.get(p) ?? p })}
                  </option>
                ))}
              </select>
            </div>
          )}
        </>
      )}
      {error && <p className="err small">{error}</p>}
      <Checklist op={op} />
      {op?.state === "done" && <p className="ok small">{t("tun.done")}</p>}
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: describeError(t, op.error ?? "", names) })}</p>}
    </Dialog>
  );
}

/** The bytes a link of this kind adds to every packet (GRE with its key, or VXLAN). */
function overhead(l: Link): number {
  return l.kind === "vxlan" ? 50 : 28;
}

/** The MTU a link's interfaces have: its own, or the most a 1500-byte path allows. */
export function linkMtu(l: Link): number {
  return l.mtu > 0 ? l.mtu : 1500 - overhead(l);
}

/** A link's MTU: measured again, set by hand, or back to the default. */
function MtuDialog({ link, servers, onClose, onDone }: { link: Link; servers: ServerInfo[]; onClose: () => void; onDone: () => void }) {
  const { t, num, toast } = useApp();
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const name = (id: string) => names.get(id) ?? id;
  const [now, setNow] = useState({ mtu: linkMtu(link), how: link.mtu_how });
  const [own, setOwn] = useState(String(linkMtu(link)));
  const [busy, setBusy] = useState<"" | "measure" | "save">("");
  const [path, setPath] = useState<number | null>(null);
  const [error, setError] = useState("");
  const most = 1500 - overhead(link);
  const ownNum = Number(own.trim());
  const ownOk = /^\d{3,4}$/.test(own.trim()) && ownNum >= 576 && ownNum <= most;
  const apply = async (choice: { measure: true } | { mtu: number }) => {
    setBusy("measure" in choice ? "measure" : "save");
    setError("");
    try {
      const r = await api.setLinkMtu(link.id, choice);
      setNow({ mtu: r.mtu, how: r.how });
      setOwn(String(r.mtu));
      setPath(r.path);
      toast(r.later.length ? t("net.mtuLater", { mtu: num(r.mtu), servers: r.later.map(name).join(", ") }) : t("net.mtuSaved", { mtu: num(r.mtu) }), "ok");
      onDone();
    } catch (e) {
      setError(e instanceof ApiError ? describeError(t, e.code, names) : String(e));
    } finally {
      setBusy("");
    }
  };
  return (
    <Dialog
      title={t("net.mtuTitle", { a: name(link.a), b: name(link.b) })}
      onClose={busy ? () => {} : onClose}
      footer={
        <button className="btn btn-ghost btn-sm" type="button" disabled={!!busy} onClick={onClose}>
          {t("wz.close")}
        </button>
      }
    >
      <p className="muted small" style={{ marginTop: 0 }}>
        {t("net.mtuText", { bytes: num(overhead(link)) })}
      </p>
      <p className="mtu-now">
        <b dir="ltr">{t("net.mtuNow", { mtu: num(now.mtu) })}</b>
        {now.how && <span className="tag">{t(`net.mtuHow.${now.how}`)}</span>}
      </p>
      <div className="mtu-acts">
        <button className="btn btn-ghost btn-sm" type="button" disabled={!!busy} onClick={() => void apply({ measure: true })}>
          <Icon name="refresh" size={16} />
          {busy === "measure" ? t("net.mtuMeasuring") : t("net.mtuMeasure")}
        </button>
        {(now.how || now.mtu !== most) && (
          <button className="btn btn-quiet btn-sm" type="button" disabled={!!busy} onClick={() => void apply({ mtu: 0 })}>
            {t("net.mtuDefault", { mtu: num(most) })}
          </button>
        )}
      </div>
      {path !== null && <p className="ok small">{t("net.mtuPath", { path: num(path), mtu: num(now.mtu) })}</p>}
      <div className="field">
        <label htmlFor="mtu-own">{t("net.mtuOwn")}</label>
        <div className="mtu-own">
          <input className="text mono" id="mtu-own" dir="ltr" inputMode="numeric" value={own} onChange={(e) => setOwn(e.target.value)} />
          <button className="btn btn-primary btn-sm" type="button" disabled={!!busy || !ownOk || ownNum === now.mtu} onClick={() => void apply({ mtu: ownNum })}>
            {t("net.mtuSave")}
          </button>
        </div>
        <span className="help" dir="ltr">
          576 – {most}
        </span>
      </div>
      {error && (
        <div className="banner warn del-why" role="alert">
          <Icon name="info" size={18} />
          <span>{error}</span>
        </div>
      )}
    </Dialog>
  );
}

/** What a delete takes: one link, or a network with every link in it. */
type Doomed = { link: Link } | { network: Network; links: Link[] };

/**
 * Deleting a link or a network. The panel deletes it at once whatever state the servers are
 * in; the dialog says beforehand which servers remove the interface now and which when they
 * connect. A link that something still runs through is named with a way out: the tunnel to
 * open, or (for an agent whose server is offline, and may be gone for good) delete anyway.
 */
function DeleteNet({ what, servers, onClose, onDone, onOpenTunnel }: { what: Doomed; servers: ServerInfo[]; onClose: () => void; onDone: () => void; onOpenTunnel: (name: string) => void }) {
  const { t, toast } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const name = (id: string) => names.get(id) ?? id;
  const online = (id: string) => !!servers.find((s) => s.id === id)?.online;
  const links = "link" in what ? [what.link] : what.links;
  const ends = [...new Set(links.flatMap((l) => [l.a, l.b]))];
  const [code, arg = ""] = error.split(":");
  const canForce = code === "agent_uses" && !online(arg);
  const go = async (force: boolean) => {
    setBusy(true);
    setError("");
    try {
      const r = "link" in what ? await api.deleteLink(what.link.id, force) : await api.deleteNetwork(what.network.id, true, force);
      toast(r.later.length ? t("net.delLater", { servers: r.later.map(name).join(", ") }) : t("net.delDone"), "ok");
      onDone();
    } catch (e) {
      setError(e instanceof ApiError ? e.code : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={"link" in what ? t("net.delLinkTitle", { a: name(what.link.a), b: name(what.link.b) }) : t("net.delNetTitle", { name: what.network.name })}
      onClose={busy ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={onClose}>
            {t("cancel")}
          </button>
          {canForce ? (
            <button className="btn btn-danger solid btn-sm" type="button" disabled={busy} onClick={() => void go(true)}>
              {t("net.force")}
            </button>
          ) : (
            <button className="btn btn-danger solid btn-sm" type="button" disabled={busy || code === "in_use" || code === "agent_uses"} onClick={() => void go(false)}>
              {t("tun.delete")}
            </button>
          )}
        </>
      }
    >
      <p className="small" style={{ marginTop: 0 }}>
        {"link" in what
          ? t("net.delLinkText", { ifname: what.link.ifname })
          : links.length
            ? t("net.delNetText", { n: links.length })
            : t("net.delNetEmpty")}
      </p>
      {ends.length > 0 && (
        <ul className="del-ends">
          {ends.map((id) => (
            <li key={id}>
              <span className={`dot ${online(id) ? "up" : "off"}`} aria-hidden="true" />
              <span className="nm">{name(id)}</span>
              <span className="muted small">{online(id) ? t("net.delNow") : t("net.delOnReturn")}</span>
            </li>
          ))}
        </ul>
      )}
      {error && (
        <div className="banner warn del-why" role="alert">
          <Icon name="info" size={18} />
          <span>
            {canForce ? t("net.forceText", { server: name(arg) }) : describeError(t, error, names)}
            {code === "in_use" && arg && (
              <>
                {" "}
                <button className="btn btn-ghost btn-sm" type="button" onClick={() => onOpenTunnel(arg)}>
                  {t("net.openTunnel", { name: arg })}
                </button>
              </>
            )}
          </span>
        </div>
      )}
    </Dialog>
  );
}

/** A server's address for the others, with a way to set it. */
function AddressRow({ server, onSaved }: { server: ServerInfo; onSaved: () => void }) {
  const { t, toast } = useApp();
  // What is used now: the address that was set, else the one the server is known by.
  const known = server.addr_default ?? "";
  const [value, setValue] = useState(server.addr ?? known ?? (server.local ? location.hostname : ""));
  const [busy, setBusy] = useState(false);
  const changed = value.trim() !== (server.addr ?? "");
  const choices = [server.ip4, server.ip6].filter((a): a is string => !!a);
  const save = async () => {
    setBusy(true);
    try {
      await api.setAddress(server.id, value.trim());
      toast(t("net.addrSaved"));
      onSaved();
    } catch {
      toast(t("net.addrBad"));
    } finally {
      setBusy(false);
    }
  };
  return (
    <li className="addr-row">
      <span className="nm">{server.name}</span>
      <div className="addr-edit">
        <input className="text mono" dir="ltr" aria-label={t("net.addrOf", { name: server.name })} placeholder="203.0.113.5" value={value} onChange={(e) => setValue(e.target.value)} />
        {choices.length > 0 && (
          <span className="addr-chips">
            {choices.map((a) => (
              <button key={a} type="button" className={`pchip as-btn ${a === value.trim() ? "on" : ""}`} dir="ltr" onClick={() => setValue(a)}>
                {a}
              </button>
            ))}
          </span>
        )}
        <span className="help">{server.addr ? t("net.addrSet") : known ? t("net.addrDefault", { addr: known }) : t("net.noAddr")}</span>
      </div>
      <span className="addr-btns">
        <button className="btn btn-ghost btn-sm" type="button" disabled={busy || !value.trim() || !changed} onClick={() => void save()}>
          {t("net.addrSave")}
        </button>
        {server.addr && (
          <button
            className="btn btn-quiet btn-sm"
            type="button"
            disabled={busy}
            onClick={() => {
              setValue(known);
              setBusy(true);
              api
                .setAddress(server.id, "")
                .then(onSaved)
                .catch(() => toast(t("net.addrBad")))
                .finally(() => setBusy(false));
            }}
          >
            {t("net.useDefault")}
          </button>
        )}
      </span>
    </li>
  );
}

export function NetworksPage({ servers, onChanged, onOpenTunnel }: { servers: ServerInfo[]; onChanged: () => void; onOpenTunnel: (name: string) => void }) {
  const { t, num } = useApp();
  const { networks, links, reload } = useNetworks();
  const [dialog, setDialog] = useState<"new" | { add: Network } | null>(null);
  const [doomed, setDoomed] = useState<Doomed | null>(null);
  const [fitting, setFitting] = useState<Link | null>(null);
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const name = (id: string) => names.get(id) ?? id;

  return (
    <div className="page is-on">
      <div className="page-band">
        <div className="summary">
          <div>
            <b>{num(networks.length)}</b>
            <span>{t("net.count")}</span>
          </div>
          <div>
            <b>{num(links.length)}</b>
            <span>{t("net.linksCount")}</span>
          </div>
        </div>
        <div className="grow" />
        <button className="btn btn-primary btn-sm" type="button" onClick={() => setDialog("new")}>
          <span className="shine" />
          <Icon name="plus" size={18} />
          {t("net.new")}
        </button>
      </div>

      <Card title={t("net.addresses")} sub={t("net.addressesHint")}>
        <ul className="addr-list">
          {servers.map((s) => (
            <AddressRow key={`${s.id}-${s.addr ?? ""}`} server={s} onSaved={onChanged} />
          ))}
        </ul>
      </Card>

      {networks.length === 0 && (
        <div className="card">
          <Empty
            icon="networks"
            title={t("net.emptyTitle")}
            text={t("net.none")}
            action={
              <button className="btn btn-primary btn-sm" type="button" onClick={() => setDialog("new")}>
                <Icon name="plus" size={18} />
                {t("net.new")}
              </button>
            }
          />
        </div>
      )}
      {networks.map((n) => {
        const mine = links.filter((l) => l.network === n.id);
        return (
          <Card
            key={n.id}
            className="net-card"
            title={
              <>
                {n.name} <span className="tag">{n.cidr}</span>{" "}
                <span className="tag" dir="ltr">
                  {kindTag(n)}
                </span>
              </>
            }
          >
            <div className="usage">
              <span>{t("net.usage", { used: num(n.links), of: num(n.capacity) })}</span>
              <div className="usage-bar" aria-hidden="true">
                <i style={{ width: `${Math.max(2, (n.links / Math.max(1, n.capacity)) * 100)}%` }} />
              </div>
            </div>
            {mine.length === 0 ? (
              <p className="muted small">{t("net.noLinks")}</p>
            ) : (
              <table className="plain-table">
                <thead>
                  <tr>
                    <th>{t("t.route")}</th>
                    <th>{t("net.addrs")}</th>
                    <th className="hide-sm">{t("net.key")}</th>
                    <th>{t("net.mtu")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {mine.map((l) => (
                    <tr key={l.id}>
                      <td>
                        {name(l.a)} <span className="muted">↔</span> {name(l.b)}
                      </td>
                      <td className="mono" dir="ltr">
                        {l.addr_a} · {l.addr_b}
                      </td>
                      <td className="mono hide-sm" dir="ltr">
                        {l.kind === "vxlan" ? `VNI ${l.gre_key}` : l.gre_key} · {l.ifname}
                      </td>
                      <td>
                        <button className="mtu-btn" type="button" aria-label={t("net.mtuTitle", { a: name(l.a), b: name(l.b) })} onClick={() => setFitting(l)}>
                          <span className="mono" dir="ltr">
                            {num(linkMtu(l))}
                          </span>
                          {l.mtu_how && <span className="muted small">{t(`net.mtuHow.${l.mtu_how}`)}</span>}
                          <Icon name="edit" size={14} />
                        </button>
                      </td>
                      <td>
                        <button className="btn btn-ghost btn-sm" type="button" onClick={() => setDoomed({ link: l })}>
                          {t("tun.delete")}
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            <div className="net-actions">
              <button className="btn btn-primary btn-sm" type="button" onClick={() => setDialog({ add: n })}>
                <Icon name="plus" size={18} />
                {t("net.add")}
              </button>
              <button className="btn btn-ghost btn-sm" type="button" onClick={() => setDoomed({ network: n, links: mine })}>
                {mine.length ? t("net.delNet", { n: num(mine.length) }) : t("tun.delete")}
              </button>
            </div>
          </Card>
        );
      })}
      <div className="banner info net-honest">
        <Icon name="info" size={18} />
        <span>
          {t("net.honest")} {t("net.honestVxlan")}
        </span>
      </div>

      {fitting && <MtuDialog link={fitting} servers={servers} onClose={() => setFitting(null)} onDone={reload} />}
      {doomed && (
        <DeleteNet
          what={doomed}
          servers={servers}
          onOpenTunnel={onOpenTunnel}
          onClose={() => setDoomed(null)}
          onDone={() => {
            setDoomed(null);
            reload();
            onChanged();
          }}
        />
      )}
      {dialog === "new" && (
        <NewNetwork
          servers={servers}
          onClose={() => {
            setDialog(null);
            reload();
          }}
        />
      )}
      {dialog && dialog !== "new" && (
        <AddLinks
          network={dialog.add}
          servers={servers}
          onClose={() => {
            setDialog(null);
            reload();
            onChanged();
          }}
        />
      )}
    </div>
  );
}
