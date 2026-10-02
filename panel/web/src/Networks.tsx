import { useEffect, useState } from "react";
import { ApiError, api } from "./api";
import type { Link, Network, ServerInfo } from "./api";
import { Checklist, describeError, useOp } from "./ops";
import { useApp } from "./store";
import { Card, Dialog, Empty, Icon } from "./ui";

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
  const [error, setError] = useState("");
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const make = async () => {
    setError("");
    try {
      await api.createNetwork(name.trim(), cidr.trim());
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
          <button className="btn btn-primary btn-sm" type="button" onClick={() => void make()}>
            {t("net.make")}
          </button>
        </>
      }
    >
      <p className="muted small">{t("net.newText")}</p>
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
          <p className="muted small">{t("net.linksText")}</p>
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

export function NetworksPage({ servers, onChanged }: { servers: ServerInfo[]; onChanged: () => void }) {
  const { t, num, toast } = useApp();
  const { networks, links, reload } = useNetworks();
  const [dialog, setDialog] = useState<"new" | { add: Network } | null>(null);
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const name = (id: string) => names.get(id) ?? id;

  const remove = async (fn: () => Promise<object>) => {
    try {
      await fn();
      reload();
      onChanged();
    } catch (e) {
      toast(e instanceof ApiError ? describeError(t, e.code, names) : String(e));
    }
  };

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
                {n.name} <span className="tag">{n.cidr}</span>
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
                      <td className="mono hide-sm">
                        {l.gre_key} · {l.ifname}
                      </td>
                      <td>
                        <button className="btn btn-ghost btn-sm" type="button" onClick={() => void remove(() => api.deleteLink(l.id))}>
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
              {mine.length === 0 && (
                <button className="btn btn-ghost btn-sm" type="button" onClick={() => void remove(() => api.deleteNetwork(n.id))}>
                  {t("tun.delete")}
                </button>
              )}
            </div>
          </Card>
        );
      })}
      <div className="banner info net-honest">
        <Icon name="info" size={18} />
        <span>{t("net.honest")}</span>
      </div>

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
