import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { ServerInfo } from "./api";
import { pairTunnels, rateParts } from "./derive";
import type { Tunnel, TunnelState } from "./derive";
import { useApp } from "./store";
import { Chart } from "./Chart";
import { Dialog, Icon, Seg } from "./ui";
import { Checklist, opError, useOp } from "./ops";
import { RouteScene } from "./RouteScene";
import { SpeedTest } from "./SpeedTest";
import { Wizard } from "./Wizard";

const stateKey = { up: "st.up", down: "st.down", off: "st.off" } as const;

/** A dialog that runs one operation (start, stop, delete, a new token) and shows it. With
 * `confirm`, the operation waits until that word is typed. */
function RunDialog({
  title,
  text,
  danger,
  label,
  confirm,
  start,
  onClose,
}: {
  title: string;
  text: string;
  danger?: boolean;
  label: string;
  confirm?: string;
  start: () => Promise<{ op: string }>;
  onClose: (done: boolean) => void;
}) {
  const { t } = useApp();
  const [id, setId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [typed, setTyped] = useState("");
  const op = useOp(id);
  const running = !!id && (!op || op.state === "running");
  const go = async () => {
    setError("");
    try {
      setId((await start()).op);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "busy" ? t("wz.busy") : t("tun.failed", { why: e instanceof ApiError ? e.code : String(e) }));
    }
  };
  const ready = !confirm || typed.trim() === confirm;
  return (
    <Dialog
      title={title}
      onClose={running ? () => {} : () => onClose(op?.state === "done")}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={running} onClick={() => onClose(op?.state === "done")}>
            {op?.state === "done" ? t("wz.close") : t("cancel")}
          </button>
          {!id && (
            <button className={`btn btn-sm ${danger ? "btn-danger solid" : "btn-primary"}`} type="button" disabled={!ready} onClick={() => void go()}>
              {label}
            </button>
          )}
        </>
      }
    >
      {!id && <p className="muted">{text}</p>}
      {!id && confirm && (
        <div className="field">
          <label htmlFor="run-confirm">{t("td.delConfirm")}</label>
          <input className="text mono" id="run-confirm" dir="ltr" autoComplete="off" placeholder={confirm} value={typed} onChange={(e) => setTyped(e.target.value)} />
        </div>
      )}
      {error && <p className="err small">{error}</p>}
      <Checklist op={op} />
      {op?.state === "done" && <p className="ok small">{t("tun.done")}</p>}
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: opError(t, op.error) })}</p>}
    </Dialog>
  );
}

function StateBadge({ state }: { state: TunnelState }) {
  const { t } = useApp();
  return (
    <span className={`state ${state}`}>
      <i className={`dot ${state}`} />
      {t(stateKey[state])}
    </span>
  );
}

function SideFacts({ side, label }: { side: Tunnel["entry"]; label: string }) {
  const { t, num } = useApp();
  if (!side) return null;
  const s = side.tunnel.status;
  return (
    <div className="review-card">
      <h4>
        {label} · {side.server.name}
      </h4>
      <dl className="kv">
        <dt>{t("t.state")}</dt>
        <dd>
          {side.tunnel.error ? (
            <span className="err">{side.tunnel.error}</span>
          ) : s ? (
            <StateBadge state={s.peer.connected ? "up" : "down"} />
          ) : side.tunnel.active === false ? (
            <StateBadge state="off" />
          ) : (
            "—"
          )}
        </dd>
        <dt>{t("wz.transport")}</dt>
        <dd className="mono">
          {side.tunnel.transport} · {side.tunnel.mode}
        </dd>
        {side.tunnel.listen && (
          <>
            <dt>{t("td.listen")}</dt>
            <dd className="mono" dir="ltr">
              {side.tunnel.listen}
            </dd>
          </>
        )}
        {side.tunnel.remote && (
          <>
            <dt>{t("td.target")}</dt>
            <dd className="mono" dir="ltr">
              {side.tunnel.remote}
            </dd>
          </>
        )}
        {s?.peer.sessions != null && (
          <>
            <dt>{t("tun.sessions")}</dt>
            <dd className="num">{num(s.peer.sessions)}</dd>
          </>
        )}
      </dl>
      {s?.peer.last_error && !s.peer.connected && <p className="err small">{s.peer.last_error.text}</p>}
    </div>
  );
}

function bytes(n: number): { value: number; unit: string; decimals: number } {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return { value: v, unit: units[i], decimals: i === 0 || v >= 100 ? 0 : 1 };
}

type Action = "start" | "stop" | "restart" | "delete" | "rotate";

/** One tunnel, on a page of its own. */
function TunnelPage({ tunnel, onBack, onAct, onEdit }: { tunnel: Tunnel; onBack: () => void; onAct: (a: Action) => void; onEdit: () => void }) {
  const { t, num } = useApp();
  const speed = useRef<HTMLDivElement>(null);
  const forwards = tunnel.entry?.tunnel.forwards ?? tunnel.exit?.tunnel.forwards ?? [];
  const status = tunnel.entry?.tunnel.status ?? null;
  const up = tunnel.state === "up";
  const r = rateParts(tunnel.rate);
  const moved = bytes(status ? status.totals.bytes_up + status.totals.bytes_down : 0);
  const mode = tunnel.entry?.tunnel.mode ?? tunnel.exit?.tunnel.mode ?? "";

  useEffect(() => {
    document.querySelector(".main")?.scrollTo({ top: 0 });
  }, [tunnel.name]);

  return (
    <div className="page is-on tunnel-page">
      <div className="hero-route">
        <div className="head">
          <button className="btn btn-ghost btn-sm td-back" type="button" onClick={onBack}>
            <Icon name="back" size={18} />
            {t("td.back")}
          </button>
          <h2 className="td-name mono">{tunnel.name}</h2>
          <StateBadge state={tunnel.state} />
          <span className="tag">{tunnel.transport}</span>
          <span className="tag">{tunnel.profile}</span>
          {mode && <span className="tag">{t(`td.mode.${mode}`)}</span>}
          <span className="grow" />
          <div className="row-actions">
            <button className="btn btn-ghost btn-sm" type="button" disabled={!up} onClick={() => speed.current?.scrollIntoView({ behavior: "smooth", block: "start" })}>
              <Icon name="bolt" size={18} />
              {t("sp.title")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" disabled={tunnel.state === "off"} onClick={() => onAct("restart")}>
              <Icon name="restart" size={18} />
              {t("tun.restart")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct(tunnel.state === "off" ? "start" : "stop")}>
              <Icon name={tunnel.state === "off" ? "play" : "pause"} size={18} />
              {t(tunnel.state === "off" ? "tun.start" : "tun.stop")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" disabled={!tunnel.paired} onClick={() => onAct("rotate")}>
              <Icon name="key" size={18} />
              {t("tun.rotate")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" disabled={!tunnel.paired} title={tunnel.paired ? undefined : t("wz.oneSide")} onClick={onEdit}>
              <Icon name="edit" size={18} />
              {t("tun.edit")}
            </button>
            <button className="btn btn-danger btn-sm" type="button" onClick={() => onAct("delete")}>
              <Icon name="trash" size={18} />
              {t("tun.delete")}
            </button>
          </div>
        </div>
        <RouteScene tunnel={tunnel} />
      </div>

      {tunnel.state === "down" && (
        <div className="diagnosis" role="alert">
          <div className="diag-icon">!</div>
          <div>
            <h3>{t("td.diagDown")}</h3>
            <p>{t("td.diagText")}</p>
            {tunnel.error && (
              <p className="mono small" dir="auto">
                {t("td.diagLast", { e: tunnel.error })}
              </p>
            )}
            <ol>
              <li>{t("td.diag1")}</li>
              <li>{t("td.diag2")}</li>
            </ol>
          </div>
          <button className="btn btn-primary btn-sm" type="button" onClick={() => onAct("restart")}>
            <span className="shine" />
            <Icon name="restart" size={18} />
            {t("tun.restart")}
          </button>
        </div>
      )}
      {tunnel.state === "off" && (
        <div className="diagnosis calm">
          <div className="diag-icon">
            <Icon name="pause" size={18} />
          </div>
          <div>
            <h3>{t("td.diagOff")}</h3>
            <p>{t("td.diagOffText")}</p>
          </div>
          <button className="btn btn-primary btn-sm" type="button" onClick={() => onAct("start")}>
            <span className="shine" />
            <Icon name="play" size={18} />
            {t("tun.start")}
          </button>
        </div>
      )}
      {!tunnel.paired && <p className="td-note muted small">{t("td.oneSide")}</p>}

      <div className="stratum s1">
        <div className="facts">
          <div className="fact">
            <div className="label">{t("td.rate")}</div>
            <div className="value">
              {up ? num(r.value, r.decimals) : "—"}
              {up && <small>{r.unit}</small>}
            </div>
          </div>
          <div className="fact">
            <div className="label">{t("tun.rtt")}</div>
            <div className="value">
              {up && tunnel.rtt != null ? num(tunnel.rtt, 0) : "—"}
              {up && tunnel.rtt != null && <small>ms</small>}
            </div>
          </div>
          <div className="fact">
            <div className="label">{t("tun.conns")}</div>
            <div className="value">{up ? num(tunnel.connections) : "—"}</div>
          </div>
          <div className="fact">
            <div className="label">{t("tun.sessions")}</div>
            <div className="value">{status?.peer.sessions != null ? num(status.peer.sessions) : "—"}</div>
          </div>
          <div className="fact">
            <div className="label">{t("td.moved")}</div>
            <div className="value">
              {status ? num(moved.value, moved.decimals) : "—"}
              {status && <small>{moved.unit}</small>}
            </div>
          </div>
        </div>
      </div>

      <div className="stratum s2 td-speed" ref={speed}>
        <div className="stratum-head">
          <h2>{t("sp.title")}</h2>
        </div>
        <SpeedTest tunnel={tunnel} />
      </div>

      {tunnel.entry && (
        <div className="stratum s1">
          <div className="stratum-head">
            <h2>{t("td.traffic")}</h2>
          </div>
          <div className="grid-3">
            <Chart series={`tun:${tunnel.name}:rate`} unit="Mbps" />
            <Chart series={`tun:${tunnel.name}:rtt`} unit="ms" decimals={0} />
            <Chart series={`tun:${tunnel.name}:conns`} unit="" decimals={0} />
          </div>
        </div>
      )}

      <div className="stratum s2">
        <div className="grid-2">
          <div>
            <div className="stratum-head">
              <h2>{t("tun.forwards")}</h2>
            </div>
            {forwards.length === 0 ? (
              <p className="muted small">—</p>
            ) : (
              <table className="plain-table">
                <thead>
                  <tr>
                    <th>{t("td.listen")}</th>
                    <th>{t("td.target")}</th>
                    <th>{t("td.proto")}</th>
                  </tr>
                </thead>
                <tbody>
                  {forwards.map((f) => (
                    <tr key={`${f.listen}-${f.target}-${f.protocol}`}>
                      <td className="mono">{f.listen}</td>
                      <td className="mono">{f.target}</td>
                      <td>
                        <span className="tag">{f.protocol}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <div>
            <div className="stratum-head">
              <h2>{t("td.sides")}</h2>
            </div>
            <div className="td-sides">
              <SideFacts side={tunnel.entry} label={t("tun.side.entry")} />
              <SideFacts side={tunnel.exit} label={t("tun.side.exit")} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

type Filter = "all" | TunnelState;

export function TunnelsPage({ servers, onChanged }: { servers: ServerInfo[]; onChanged: () => void }) {
  const { t, num } = useApp();
  const tunnels = pairTunnels(servers);
  const online = servers.filter((s) => s.online);
  const [wizard, setWizard] = useState<{ edit?: Tunnel } | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [act, setAct] = useState<{ name: string; action: Action } | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const openTunnel = tunnels.find((x) => x.name === open) ?? null;
  const count = (s: TunnelState) => tunnels.filter((x) => x.state === s).length;

  // A tunnel that was deleted (here or elsewhere) takes the page back to the list.
  useEffect(() => {
    if (open && servers.length && !openTunnel && !act) setOpen(null);
  }, [open, openTunnel, servers.length, act]);

  const q = query.trim().toLowerCase();
  const rows = tunnels.filter(
    (x) =>
      (filter === "all" || x.state === filter) &&
      (!q ||
        [x.name, x.entry?.server.name, x.exit?.server.name, x.transport, ...(x.entry?.tunnel.forwards ?? []).map((f) => f.listen)]
          .join(" ")
          .toLowerCase()
          .includes(q)),
  );

  const dialogs = (
    <>
      {act && (
        <RunDialog
          key={`${act.name}-${act.action}`}
          title={t(act.action === "delete" ? "tun.deleteTitle" : act.action === "rotate" ? "tun.rotateTitle" : `tun.${act.action}`, { name: act.name })}
          text={act.action === "delete" ? t("tun.deleteText") : act.action === "rotate" ? t("tun.rotateText") : `${act.name}`}
          danger={act.action === "delete"}
          confirm={act.action === "delete" ? act.name : undefined}
          label={t(act.action === "delete" ? "tun.delete" : act.action === "rotate" ? "tun.rotate" : `tun.${act.action}`)}
          start={async () => {
            if (act.action === "delete") return api.deleteTunnel(act.name);
            if (act.action === "rotate") {
              const x = tunnels.find((y) => y.name === act.name);
              if (!x?.entry || !x.exit) throw new ApiError(400, { error: "no_such_tunnel" });
              const [a, b] = await Promise.all([api.tunnelSpec(x.entry.server.id, x.name), api.tunnelSpec(x.exit.server.id, x.name)]);
              const acceptor = x.entry.tunnel.mode === "reverse" ? a : b;
              const dialer = x.entry.tunnel.mode === "reverse" ? b : a;
              return api.editTunnel({
                name: x.name,
                entry: x.entry.server.id,
                exit: x.exit.server.id,
                mode: a.mode,
                transport: a.transport,
                profile: a.profile,
                listen: acceptor.listen ?? "",
                dial: dialer.remote ?? "",
                pool: a.pool,
                ws_path: a.ws_path,
                ws_host: a.ws_host,
                tls_sni: dialer.tls_sni,
                forwards: a.forwards,
                rotate: true,
              });
            }
            return api.controlTunnel(act.name, act.action as "start" | "stop" | "restart");
          }}
          onClose={(done) => {
            if (done && act.action === "delete") setOpen(null);
            setAct(null);
            onChanged();
          }}
        />
      )}
      {wizard && (
        <Wizard
          servers={servers}
          edit={wizard.edit}
          onClose={() => {
            setWizard(null);
            onChanged();
          }}
        />
      )}
    </>
  );

  if (openTunnel) {
    return (
      <>
        <TunnelPage
          tunnel={openTunnel}
          onBack={() => setOpen(null)}
          onAct={(a) => setAct({ name: openTunnel.name, action: a })}
          onEdit={() => setWizard({ edit: openTunnel })}
        />
        {dialogs}
      </>
    );
  }

  return (
    <div className="page is-on">
      <div className="page-band">
        <Seg
          value={filter}
          options={(["all", "up", "down", "off"] as Filter[]).map((f) => [f, `${f === "all" ? t("tl.all") : t(stateKey[f])} ${num(f === "all" ? tunnels.length : count(f))}`] as [Filter, string])}
          onChange={setFilter}
        />
        <label className="search">
          <Icon name="search" size={18} />
          <input type="search" placeholder={t("tl.search")} aria-label={t("tl.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
        </label>
        <div className="grow" />
        <button className="btn btn-primary btn-sm" type="button" disabled={online.length < 2} title={online.length < 2 ? t("tun.needTwo") : undefined} onClick={() => setWizard({})}>
          <span className="shine" />
          <Icon name="plus" size={18} />
          {t("tun.new")}
        </button>
      </div>
      <div className="stratum s1">
        {tunnels.length === 0 ? (
          <p className="muted" style={{ margin: 0, maxWidth: 640 }}>
            {online.length < 2 ? t("tun.needTwo") : t("tun.empty")}
          </p>
        ) : (
          <table className="tunnels">
            <thead>
              <tr>
                <th>{t("t.name")}</th>
                <th>{t("t.route")}</th>
                <th>{t("t.transport")}</th>
                <th className="hide-sm">{t("t.mode")}</th>
                <th className="hide-sm">{t("t.ports")}</th>
                <th>{t("t.state")}</th>
                <th>{t("t.rate")}</th>
                <th>
                  <span className="sr-only">{t("tun.start")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.length === 0 && (
                <tr>
                  <td colSpan={8} className="muted" style={{ textAlign: "center", padding: "var(--sp-7)" }}>
                    {t("tl.none")}
                  </td>
                </tr>
              )}
              {rows.map((x) => {
                const r = rateParts(x.rate);
                const mode = x.entry?.tunnel.mode ?? x.exit?.tunnel.mode ?? "";
                return (
                  <tr key={x.name} className="clickable" onClick={() => setOpen(x.name)}>
                    <td>
                      <button className="linklike t-name" type="button" onClick={() => setOpen(x.name)}>
                        {x.name}
                      </button>
                    </td>
                    <td>
                      <span className="t-route">
                        {x.paired ? (
                          <>
                            <span>{x.entry!.server.name}</span>
                            <Icon name="arrow" size={16} />
                            <span>{x.exit!.server.name}</span>
                          </>
                        ) : (
                          <span>
                            {(x.entry ?? x.exit)!.server.name} · {t("t.oneSide")}
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="c-transport">
                      <span className="tag">{x.transport}</span> <span className="tag">{x.profile}</span>
                    </td>
                    <td className="c-transport hide-sm">{mode ? t(`td.mode.${mode}`) : "—"}</td>
                    <td className="c-transport num hide-sm">{num((x.entry ?? x.exit)?.tunnel.forwards.length ?? 0)}</td>
                    <td>
                      <StateBadge state={x.state} />
                    </td>
                    <td className="rate num">{x.state === "up" ? `${num(r.value, r.decimals)} ${r.unit}` : "—"}</td>
                    <td>
                      <button
                        className="switch"
                        type="button"
                        role="switch"
                        aria-checked={x.state !== "off"}
                        aria-label={t("tl.toggle", { name: x.name })}
                        onClick={(e) => {
                          e.stopPropagation();
                          setAct({ name: x.name, action: x.state === "off" ? "start" : "stop" });
                        }}
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
      {dialogs}
    </div>
  );
}
