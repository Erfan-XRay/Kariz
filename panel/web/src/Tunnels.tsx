import { useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { ServerInfo } from "./api";
import { pairTunnels, rateParts } from "./derive";
import type { Tunnel } from "./derive";
import { useApp } from "./store";
import { Chart } from "./Chart";
import { SpeedDialog } from "./Extras";
import { Dialog, Icon } from "./ui";
import { Checklist, opError, useOp } from "./ops";
import { Wizard } from "./Wizard";

const stateKey = { up: "st.up", down: "st.down", off: "st.off" } as const;

/** A dialog that runs one operation (start, stop, delete, a new token) and shows it. */
function RunDialog({
  title,
  text,
  danger,
  label,
  start,
  onClose,
}: {
  title: string;
  text: string;
  danger?: boolean;
  label: string;
  start: () => Promise<{ op: string }>;
  onClose: () => void;
}) {
  const { t } = useApp();
  const [id, setId] = useState<string | null>(null);
  const [error, setError] = useState("");
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
  return (
    <Dialog
      title={title}
      onClose={running ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={running} onClick={onClose}>
            {op?.state === "done" ? t("wz.close") : t("cancel")}
          </button>
          {!id && (
            <button className={`btn btn-sm ${danger ? "btn-danger" : "btn-primary"}`} type="button" onClick={() => void go()}>
              {label}
            </button>
          )}
        </>
      }
    >
      {!id && <p className="muted">{text}</p>}
      {error && <p className="err small">{error}</p>}
      <Checklist op={op} />
      {op?.state === "done" && <p className="ok small">{t("tun.done")}</p>}
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: opError(t, op.error) })}</p>}
    </Dialog>
  );
}

function Facts({ side, label }: { side: Tunnel["entry"]; label: string }) {
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
            <span className={`state ${s.peer.connected ? "up" : "down"}`}>
              <i className={`dot ${s.peer.connected ? "up" : "down"}`} />
              {t(s.peer.connected ? "st.up" : "st.down")}
            </span>
          ) : (
            "—"
          )}
        </dd>
        {s?.peer.rtt_ms != null && (
          <>
            <dt>{t("tun.rtt")}</dt>
            <dd className="num">{num(s.peer.rtt_ms, 0)} ms</dd>
          </>
        )}
        {s?.peer.sessions != null && (
          <>
            <dt>{t("tun.sessions")}</dt>
            <dd className="num">{num(s.peer.sessions)}</dd>
          </>
        )}
        <dt>{t("wz.transport")}</dt>
        <dd className="mono">
          {side.tunnel.transport} · {side.tunnel.mode}
        </dd>
        {side.tunnel.listen && (
          <>
            <dt>{t("wz.port", { server: "" }).trim()}</dt>
            <dd className="mono" dir="ltr">
              {side.tunnel.listen}
            </dd>
          </>
        )}
        {side.tunnel.remote && (
          <>
            <dt>→</dt>
            <dd className="mono" dir="ltr">
              {side.tunnel.remote}
            </dd>
          </>
        )}
      </dl>
      {s?.peer.last_error && !s.peer.connected && <p className="err small">{s.peer.last_error.text}</p>}
    </div>
  );
}

type Action = "start" | "stop" | "restart" | "delete" | "rotate" | "speed";

function Detail({ tunnel, onClose, onAct, onEdit }: { tunnel: Tunnel; onClose: () => void; onAct: (a: Action) => void; onEdit: () => void }) {
  const { t, num } = useApp();
  const forwards = tunnel.entry?.tunnel.forwards ?? [];
  return (
    <Dialog
      title={tunnel.name}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct("delete")}>
            {t("tun.delete")}
          </button>
          <span className="grow" />
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct("speed")}>
            {t("speed.run")}
          </button>
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct("rotate")}>
            {t("tun.rotate")}
          </button>
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct(tunnel.state === "off" ? "start" : "restart")}>
            {t(tunnel.state === "off" ? "tun.start" : "tun.restart")}
          </button>
          {tunnel.state !== "off" && (
            <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct("stop")}>
              {t("tun.stop")}
            </button>
          )}
          <button className="btn btn-primary btn-sm" type="button" disabled={!tunnel.paired} title={tunnel.paired ? undefined : t("wz.oneSide")} onClick={onEdit}>
            {t("tun.edit")}
          </button>
        </>
      }
    >
      <div className="review">
        <Facts side={tunnel.entry} label={t("tun.side.entry")} />
        <Facts side={tunnel.exit} label={t("tun.side.exit")} />
      </div>
      {forwards.length > 0 && (
        <div className="field">
          <span className="label">{t("tun.forwards")}</span>
          <div className="port-chips">
            {forwards.map((f) => (
              <span className="pchip" key={`${f.listen}-${f.target}`}>
                {f.listen} → {f.target} <small>{f.protocol}</small>
              </span>
            ))}
          </div>
        </div>
      )}
      {tunnel.entry && (
        <div className="review">
          <Chart series={`tun:${tunnel.name}:rate`} unit="Mbps" />
          <Chart series={`tun:${tunnel.name}:rtt`} unit="ms" decimals={0} />
        </div>
      )}
      <p className="muted small">
        {t("t.rate")}: {tunnel.state === "up" ? `${num(rateParts(tunnel.rate).value, rateParts(tunnel.rate).decimals)} ${rateParts(tunnel.rate).unit}` : "—"}
      </p>
    </Dialog>
  );
}

export function TunnelsPage({ servers, onChanged }: { servers: ServerInfo[]; onChanged: () => void }) {
  const { t, num } = useApp();
  const tunnels = pairTunnels(servers);
  const online = servers.filter((s) => s.online);
  const [wizard, setWizard] = useState<{ edit?: Tunnel } | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [act, setAct] = useState<{ name: string; action: Action } | null>(null);
  const openTunnel = tunnels.find((x) => x.name === open) ?? null;
  const last = useRef<Tunnel | null>(null);
  if (openTunnel) last.current = openTunnel;

  const run = (name: string, action: Action) => {
    setAct({ name, action });
  };

  return (
    <div className="page is-on">
      <div className="page-band">
        <div className="summary">
          <div>
            <b>{num(tunnels.length)}</b>
            <span>{t("m.tunnels")}</span>
          </div>
          <div>
            <b>{num(tunnels.filter((x) => x.state === "up").length)}</b>
            <span>{t("st.up")}</span>
          </div>
        </div>
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
                <th>{t("t.state")}</th>
                <th>{t("t.rate")}</th>
              </tr>
            </thead>
            <tbody>
              {tunnels.map((x) => {
                const r = rateParts(x.rate);
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
                    <td>
                      <span className={`state ${x.state}`}>
                        <i className={`dot ${x.state}`} />
                        {t(stateKey[x.state])}
                      </span>
                    </td>
                    <td className="rate num">{x.state === "up" ? `${num(r.value, r.decimals)} ${r.unit}` : "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      {openTunnel && !act && !wizard && (
        <Detail
          tunnel={openTunnel}
          onClose={() => setOpen(null)}
          onAct={(a) => run(openTunnel.name, a)}
          onEdit={() => setWizard({ edit: openTunnel })}
        />
      )}
      {act?.action === "speed" && <SpeedDialog name={act.name} onClose={() => setAct(null)} />}
      {act && act.action !== "speed" && (
        <RunDialog
          key={`${act.name}-${act.action}`}
          title={t(act.action === "delete" ? "tun.deleteTitle" : act.action === "rotate" ? "tun.rotateTitle" : `tun.${act.action}`, { name: act.name })}
          text={act.action === "delete" ? t("tun.deleteText") : act.action === "rotate" ? t("tun.rotateText") : `${act.name}`}
          danger={act.action === "delete"}
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
          onClose={() => {
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
            setOpen(null);
            onChanged();
          }}
        />
      )}
    </div>
  );
}
