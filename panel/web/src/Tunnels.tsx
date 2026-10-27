import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { ApiError, api, isPrivatePath } from "./api";
import type { ServerInfo } from "./api";
import { pairTunnels, rateParts } from "./derive";
import type { Tunnel, TunnelState } from "./derive";
import { useApp } from "./store";
import { Chart } from "./Chart";
import { Card, Dialog, Empty, Icon, Menu, Seg, Sep, Skeleton, Stat, Tabs, useAgo, useSpan } from "./ui";
import { ArrangeList, arranged } from "./Arrange";
import type { ArrangeItem } from "./Arrange";

/** The longest label a tunnel can be given (as the panel checks it). */
const MAX_LABEL = 48;

/** Gives a tunnel a name to be shown by. Its own name, which its files and services carry, stays. */
function RenameDialog({ tunnel, onClose, onSaved }: { tunnel: Tunnel; onClose: () => void; onSaved: () => void }) {
  const { t, num, toast } = useApp();
  const [label, setLabel] = useState(tunnel.label ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const clean = label.trim().replace(/\s+/g, " ");
  const long = [...clean].length > MAX_LABEL;
  const same = clean === (tunnel.label ?? "");
  const save = async (value: string) => {
    setBusy(true);
    setError("");
    try {
      await api.labelTunnel(tunnel.name, value);
      toast(value ? t("rn.saved", { name: value }) : t("rn.cleared", { name: tunnel.name }), "ok");
      onSaved();
    } catch {
      setBusy(false);
      setError(t("rn.failed"));
    }
  };
  return (
    <Dialog
      title={t("rn.title")}
      onClose={onClose}
      footer={
        <>
          {tunnel.label && (
            <button className="btn btn-quiet btn-sm" type="button" disabled={busy} onClick={() => save("")}>
              {t("rn.clear")}
            </button>
          )}
          <span className="grow" />
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="submit" form="rn-form" disabled={busy || long || same}>
            {t("rn.save")}
          </button>
        </>
      }
    >
      <form
        id="rn-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!busy && !long && !same) save(clean);
        }}
      >
        <div className="field">
          <label htmlFor="rn-label">{t("rn.label")}</label>
          <input
            className="text"
            id="rn-label"
            dir="auto"
            autoFocus
            autoComplete="off"
            placeholder={tunnel.name}
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            aria-describedby="rn-help"
          />
          <span className="help" id="rn-help">
            {t("rn.help", { name: tunnel.name })}
          </span>
          <span className="err">{long ? t("rn.long", { n: num(MAX_LABEL) }) : error}</span>
        </div>
      </form>
    </Dialog>
  );
}
import { Checklist, opError, useOp } from "./ops";
import { RouteScene } from "./RouteScene";
import { SpeedTest } from "./SpeedTest";
import { BenchPanel, isV6 } from "./Benchmark";
import type { BenchChoice } from "./Benchmark";
import { useNetworks } from "./Networks";
import { linkWithAddress } from "./GreChoice";
import { AutoRestartBlock } from "./AutoRestart";
import { Wizard } from "./Wizard";
import { TunnelEdit } from "./TunnelEdit";
import { transportLabel } from "./transport";
import { ResetUsage, UsagePanel, useUsage, volume } from "./Usage";
import { causeOf } from "./cause";

const stateKey = { up: "st.up", down: "st.down", off: "st.off" } as const;

/** A dialog that runs one operation (start, stop, delete, a new token) and shows it. With
 * `confirm`, the operation waits until that word is typed. With `autoClose`, it closes by
 * itself a moment after the operation is done, unless there is something to read (a server
 * that was offline). */
function RunDialog({
  title,
  text,
  danger,
  label,
  confirm,
  autoClose,
  start,
  onClose,
  names,
}: {
  names: Map<string, string>;
  title: string;
  text: string;
  danger?: boolean;
  label: string;
  confirm?: string;
  autoClose?: boolean;
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
  const closeNow = useRef(onClose);
  closeNow.current = onClose;
  const finished = op?.state === "done" && !op.offline?.length;
  useEffect(() => {
    if (!autoClose || !finished) return;
    const timer = setTimeout(() => closeNow.current(true), 1200);
    return () => clearTimeout(timer);
  }, [autoClose, finished]);
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
      {op?.state === "done" && !!op.offline?.length && (
        <p className="warn-text small" role="status">
          {t(op.kind === "delete" ? "tun.deleteLater" : "tun.offlineSkipped", { servers: op.offline.map((id) => names.get(id) ?? id).join(", ") })}
        </p>
      )}
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: opError(t, op.error, names) })}</p>}
    </Dialog>
  );
}

function StateBadge({ state }: { state: TunnelState }) {
  const { t } = useApp();
  // A state that just changed (broke, came back, was stopped) rings once, so the eye finds it.
  const was = useRef(state);
  const [rang, setRang] = useState(false);
  useEffect(() => {
    if (was.current === state) return;
    was.current = state;
    setRang(true);
    const id = setTimeout(() => setRang(false), 1200);
    return () => clearTimeout(id);
  }, [state]);
  return (
    <span className={`pill ${state} ${rang ? "is-new" : ""}`}>
      <i className={`dot ${state}`} aria-hidden="true" />
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
          {transportLabel(side.tunnel.transport, side.tunnel.status?.peer.transport)} · {side.tunnel.mode}
        </dd>
        {side.tunnel.encryption && (
          <>
            <dt>{t("enc.title")}</dt>
            <dd className={side.tunnel.encryption === "none" ? "err" : undefined}>{side.tunnel.transport === "quic" ? "TLS 1.3" : t(`enc.${side.tunnel.encryption}`)}</dd>
          </>
        )}
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
      {s?.peer.last_error && !s.peer.connected && <ErrorWhy text={s.peer.last_error.text} />}
    </div>
  );
}

/** A side's last error as it said it, and under it what that likely means. */
function ErrorWhy({ text }: { text: string }) {
  const { t } = useApp();
  const cause = causeOf(text);
  return (
    <>
      <p className="err small" dir="ltr">
        {text}
      </p>
      {cause && <p className="muted small">{t("cause.likely", { why: t(`cause.${cause}`) })}</p>}
    </>
  );
}

/** What the entry side's health probe found, and how full the tunnel's connections are. */
function HealthBlock({ tunnel }: { tunnel: Tunnel }) {
  const { t, num } = useApp();
  const ago = useAgo();
  const p = tunnel.probe;
  const target = p?.target || t("td.health.tunnelOnly");
  let line: ReactNode;
  if (!p) {
    line = <span className="muted">{t("td.health.none")}</span>;
  } else if (p.outcome === "ok") {
    line = (
      <span>
        <b className="ok">{t("td.health.ok")}</b> · <span dir="ltr">{num(p.ms ?? 0)} ms</span> · <span className="mono" dir="ltr">{target}</span>
      </span>
    );
  } else if (p.outcome === "target_failed") {
    line = <b className="err">{t("td.health.target", { target })}</b>;
  } else if (p.outcome === "failed") {
    line = <b className="err">{t("td.health.failed")}</b>;
  } else {
    line = <span className="muted">{t("td.health.old")}</span>;
  }
  const c = tunnel.capacity;
  const pct = c ? Math.min(100, (c.streams / c.max) * 100) : 0;
  return (
    <div className="grid-2 td-health">
      <div className="field">
        <span className="label">{t("td.health.title")}</span>
        <p className="small" aria-live="polite">
          {line}
        </p>
        {p && p.outcome !== "unsupported" && (
          <span className="help">
            {t("td.health.checked", { ago: ago(p.secs_ago), ok: num(p.ok_total), failed: num(p.failed_total) })}
          </span>
        )}
      </div>
      <div className="field">
        <div className={`meter ${pct >= 80 ? "hot" : ""}`}>
          <div className="top">
            <span>{t("td.capacity")}</span>
            <b dir="ltr">{c ? `${num(c.streams)} / ${num(c.max)}` : "—"}</b>
          </div>
          <div className="bar" role="progressbar" aria-label={t("td.capacity")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}>
            <i style={{ width: `${pct}%` }} />
          </div>
        </div>
        <span className="help">{c ? t("td.capacityHelp") : t("td.capacityNone")}</span>
      </div>
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
function TunnelPage({
  tunnel,
  servers,
  onBack,
  onAct,
  onEdit,
  onRename,
}: {
  tunnel: Tunnel;
  servers: ServerInfo[];
  onBack: () => void;
  onAct: (a: Action) => void;
  /** With a benchmark's choice, the edit page opens with it filled in. */
  onEdit: (preset?: BenchChoice) => void;
  onRename: () => void;
}) {
  const { t, num } = useApp();
  const span = useSpan();
  const [tab, setTab] = useState<TunnelTab>("overview");
  const { links } = useNetworks();
  const forwards = tunnel.entry?.tunnel.forwards ?? tunnel.exit?.tunnel.forwards ?? [];
  const status = tunnel.entry?.tunnel.status ?? null;
  const up = tunnel.state === "up";
  const r = rateParts(tunnel.rate);
  const moved = bytes(status ? status.totals.bytes_up + status.totals.bytes_down : 0);
  const mode = tunnel.entry?.tunnel.mode ?? tunnel.exit?.tunnel.mode ?? "";
  // What the tunnel runs over now, as the benchmark names it: its core transport (tcp is measured with mux), its
  // direction, and whether it listens on a private network link.
  const acceptorSide = mode === "reverse" ? tunnel.entry : tunnel.exit;
  const listenHost = (acceptorSide?.tunnel.listen ?? "").replace(/:\d+$/, "").replace(/^\[|\]$/g, "");
  const overLink = linkWithAddress(links, listenHost);
  const overGre = !!overLink;
  const nowCore = tunnel.transport === "tcp" ? "tcpmux" : tunnel.transport;
  // ... whether it dials an IPv6 address, and the server name a wss tunnel sends (its own, else the WebSocket
  // host's), which the benchmark tries wss with too.
  const dialerSide = mode === "reverse" ? tunnel.exit : tunnel.entry;
  const overV6 = !overGre && isV6((dialerSide?.tunnel.remote ?? "").replace(/:\d+$/, ""));
  const [tunnelSni, setTunnelSni] = useState("");
  const dialerId = dialerSide?.server.id;
  useEffect(() => {
    setTunnelSni("");
    if (tunnel.transport !== "wss" || !dialerId) return;
    let alive = true;
    api
      .tunnelSpec(dialerId, tunnel.name)
      .then((s) => {
        const name = s.tls_sni || s.ws_host || "";
        if (alive && /[a-z]/i.test(name) && !name.includes(":")) setTunnelSni(name);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [dialerId, tunnel.name, tunnel.transport]);

  return (
    <div className="page is-on tunnel-page">
      <section className="card hero-route">
        <div className="head">
          <button className="icon-square td-back" type="button" onClick={onBack} aria-label={t("td.back")} title={t("td.back")}>
            <Icon name="back" size={18} />
          </button>
          <div className="td-title">
            <div className="td-name-row">
              <h2 className={`td-name ${tunnel.label ? "" : "mono"}`} dir="auto">
                {tunnel.title}
              </h2>
              <button className="icon-square td-rename" type="button" onClick={onRename} aria-label={t("rn.open", { name: tunnel.title })} title={t("rn.title")}>
                <Icon name="edit" size={16} />
              </button>
            </div>
            {tunnel.label && (
              <span className="td-system mono" dir="ltr" title={t("rn.system")}>
                {tunnel.name}
              </span>
            )}
            <div className="td-tags">
              <StateBadge state={tunnel.state} />
              {tunnel.uptime != null && (
                <span className="tag up-for" title={t("up.title")}>
                  <Icon name="clock" size={14} />
                  {t("up.for", { span: span(tunnel.uptime) })}
                </span>
              )}
              <span className={`tag ${tunnel.via ? "via" : ""}`} dir="ltr" title={tunnel.via ? t("td.autoNow", { via: tunnel.via.replace(/tcpmux/g, "tcp") }) : undefined}>
                {transportLabel(tunnel.transport, tunnel.via)}
              </span>
              <span className="tag">{tunnel.profile}</span>
              {mode && <span className="tag">{t(`td.mode.${mode}`)}</span>}
              {tunnel.encryption === "none" && tunnel.transport !== "quic" && <span className="tag risk">{t("enc.none")}</span>}
            </div>
          </div>
          <span className="grow" />
          <div className="row-actions">
            <button className="btn btn-ghost btn-sm" type="button" disabled={tunnel.state === "off"} onClick={() => onAct("restart")}>
              <Icon name="restart" size={18} />
              {t("tun.restart")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" onClick={() => onAct(tunnel.state === "off" ? "start" : "stop")}>
              <Icon name={tunnel.state === "off" ? "play" : "pause"} size={18} />
              {t(tunnel.state === "off" ? "tun.start" : "tun.stop")}
            </button>
            <button className="btn btn-ghost btn-sm" type="button" disabled={!tunnel.paired} title={tunnel.paired ? undefined : t("wz.oneSide")} onClick={() => onEdit()}>
              <Icon name="edit" size={18} />
              {t("tun.edit")}
            </button>
            <Menu
              label={t("more")}
              items={[
                { label: t("rn.title"), icon: "edit", run: onRename },
                { label: t("tun.rotate"), icon: "key", disabled: !tunnel.paired, run: () => onAct("rotate") },
                { label: t("tun.delete"), icon: "trash", danger: true, run: () => onAct("delete") },
              ]}
            />
          </div>
        </div>
        <RouteScene tunnel={tunnel} />
      </section>

      {tunnel.state === "down" && (
        <div className="diagnosis" role="alert">
          <div className="diag-icon">
            <Icon name="alert" size={18} />
          </div>
          <div>
            <h3>{t("td.diagDown")}</h3>
            <p>{t("td.diagText")}</p>
            {tunnel.error && (
              <p className="mono small diag-err" dir="auto">
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
      {tunnel.state === "up" && tunnel.blocked && (
        <div className="diagnosis" role="alert">
          <div className="diag-icon">
            <Icon name="alert" size={18} />
          </div>
          <div>
            <h3>{t("td.blocked")}</h3>
            <p>{t(`td.blocked.${tunnel.blocked}`, { target: tunnel.probe?.target ?? "" })}</p>
            {tunnel.blocked === "path" && tunnel.probe?.error && (
              <p className="mono small diag-err" dir="auto">
                {t("td.diagLast", { e: tunnel.probe.error })}
              </p>
            )}
          </div>
          {tunnel.blocked === "full" ? (
            <button className="btn btn-primary btn-sm" type="button" disabled={!tunnel.paired} onClick={() => onEdit()}>
              <span className="shine" />
              <Icon name="edit" size={18} />
              {t("tun.edit")}
            </button>
          ) : (
            <button className="btn btn-primary btn-sm" type="button" onClick={() => onAct("restart")}>
              <span className="shine" />
              <Icon name="restart" size={18} />
              {t("tun.restart")}
            </button>
          )}
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
      {!tunnel.paired && (
        <div className="banner info">
          <Icon name="info" size={18} />
          <span>{t("td.oneSide")}</span>
        </div>
      )}

      <Tabs
        id="td"
        label={t("td.tabs")}
        value={tab}
        onChange={setTab}
        options={[
          ["overview", t("td.tab.overview")],
          ["usage", t("td.tab.usage")],
          ["tools", t("td.tab.tools")],
          ["schedule", t("td.tab.schedule")],
        ]}
      />

      <div className="tab-panel" role="tabpanel" id="td-panel-overview" aria-labelledby="td-tab-overview" hidden={tab !== "overview"}>
        <div className="stats five">
          <div className="card stat-card">
            <Stat label={t("td.rate")} icon="pulse" tone="water" value={up ? num(r.value, r.decimals) : "—"} unit={up ? r.unit : undefined} />
          </div>
          <div className="card stat-card">
            <Stat label={t("tun.rtt")} icon="clock" value={up && tunnel.rtt != null ? num(tunnel.rtt, 0) : "—"} unit={up && tunnel.rtt != null ? "ms" : undefined} />
          </div>
          <div className="card stat-card">
            <Stat label={t("tun.conns")} icon="link" value={up ? num(tunnel.connections) : "—"} />
          </div>
          <div className="card stat-card">
            <Stat label={t("tun.sessions")} icon="tunnels" value={status?.peer.sessions != null ? num(status.peer.sessions) : "—"} />
          </div>
          <div className="card stat-card">
            <Stat label={t("td.moved")} icon="download" value={status ? num(moved.value, moved.decimals) : "—"} unit={status ? moved.unit : undefined} />
          </div>
        </div>
  
        {tunnel.state === "up" && (
          <Card title={t("td.health.card")} sub={t("td.health.sub")}>
            <HealthBlock tunnel={tunnel} />
          </Card>
        )}
  
        {tunnel.entry && (
          <Card title={t("td.traffic")} sub={t("td.trafficSub")}>
            <div className="grid-3">
              <Chart series={`tun:${tunnel.name}:rate`} unit="Mbps" />
              <Chart series={`tun:${tunnel.name}:rtt`} unit="ms" decimals={0} />
              <Chart series={`tun:${tunnel.name}:conns`} unit="" decimals={0} />
            </div>
          </Card>
        )}
  
        <div className="grid-2">
          <Card title={t("tun.forwards")} sub={forwards.length ? t("td.portsSub", { n: num(forwards.length) }) : undefined} flush>
            {forwards.length === 0 ? (
              <p className="muted small pad">—</p>
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
          </Card>
          <Card title={t("td.sides")}>
            <div className="td-sides">
              <SideFacts side={tunnel.entry} label={t("tun.side.entry")} />
              <SideFacts side={tunnel.exit} label={t("tun.side.exit")} />
            </div>
          </Card>
        </div>
      </div>

      <div className="tab-panel" role="tabpanel" id="td-panel-tools" aria-labelledby="td-tab-tools" hidden={tab !== "tools"}>
        <Card title={t("sp.title")} sub={t("sp.sub")}>
          <SpeedTest tunnel={tunnel} />
        </Card>

        {tunnel.paired && tunnel.entry && tunnel.exit && (
          <Card title={t("bench.title")} sub={t("bench.sub")}>
            <BenchPanel
              entry={tunnel.entry.server.id}
              exit={tunnel.exit.server.id}
              profile={tunnel.profile}
              port={0}
              servers={servers}
              applyLabel={t("bench.useTunnel")}
              currentLabel={t("bench.currentTunnel")}
              context="tunnel"
              sni={tunnelSni}
              isCurrent={(c) => c.transport === nowCore && c.mode === mode && (isPrivatePath(c.path) ? c.network === overLink?.network : !overGre) && (c.path === "ipv6") === overV6 && !c.sni === !tunnelSni}
              onApply={(choice) => onEdit(choice)}
            />
          </Card>
        )}
      </div>

      <div className="tab-panel" role="tabpanel" id="td-panel-usage" aria-labelledby="td-tab-usage" hidden={tab !== "usage"}>
        <Card title={t("us.title")} sub={t("us.sub")} actions={<ResetUsage kind="tunnel" id={tunnel.name} name={tunnel.name} />}>
          {tab === "usage" && (
            <UsagePanel
              kind="tunnel"
              id={tunnel.name}
              sides={[tunnel.entry, tunnel.exit].filter((s): s is NonNullable<typeof s> => !!s).map((s) => [s.server.id, s.server.name] as [string, string])}
            />
          )}
        </Card>
      </div>

      <div className="tab-panel" role="tabpanel" id="td-panel-schedule" aria-labelledby="td-tab-schedule" hidden={tab !== "schedule"}>
        <Card title={t("ar.title")} sub={t("ar.subTunnel")}>
          <AutoRestartBlock tunnel={tunnel.name} />
        </Card>
      </div>
    </div>
  );
}

type TunnelTab = "overview" | "usage" | "tools" | "schedule";

type Filter = "all" | TunnelState;

export function TunnelsPage({
  servers,
  loaded,
  open,
  setOpen,
  onChanged,
}: {
  servers: ServerInfo[];
  loaded: boolean;
  open: string | null;
  setOpen: (name: string | null) => void;
  onChanged: () => void;
}) {
  const { t, num, toast } = useApp();
  const span = useSpan();
  const usage = useUsage();
  const paired = pairTunnels(servers);
  const online = servers.filter((s) => s.online);
  const [wizard, setWizard] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  // The order being arranged, while the page is in its Arrange mode; a saved one is shown
  // until the next reading of the servers has it too.
  const [order, setOrder] = useState<string[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState<string[] | null>(null);
  const keyed = paired.map((x) => ({ ...x, id: x.name }));
  const byName = new Map(keyed.map((x) => [x.name, x]));
  const tunnels = saved ? arranged(saved, keyed).map((n) => byName.get(n)!) : paired;
  useEffect(() => {
    if (saved && arranged(saved, keyed).join() === paired.map((x) => x.name).join()) setSaved(null);
  }, [servers, saved]); // eslint-disable-line react-hooks/exhaustive-deps
  const saveOrder = async () => {
    if (!order) return;
    const names = arranged(order, keyed);
    if (names.join() === tunnels.map((x) => x.name).join()) {
      setOrder(null);
      return;
    }
    setSaving(true);
    try {
      await api.orderTunnels(names);
      toast(t("arr.savedTunnels"), "ok");
      setSaved(names);
      setOrder(null);
    } catch {
      toast(t("arr.failed"), "err");
    }
    setSaving(false);
    onChanged();
  };
  const arrangeItems = tunnels.map(
    (x): ArrangeItem => ({
      id: x.name,
      name: x.title,
      badge: x.label && (
        <span className="t-sys mono" dir="ltr">
          {x.name}
        </span>
      ),
      sub: x.paired ? (
        <span className="t-route">
          <span>{x.entry!.server.name}</span>
          <Icon name="arrow" size={14} />
          <span>{x.exit!.server.name}</span>
        </span>
      ) : (
        (x.entry ?? x.exit)!.server.name
      ),
      state: x.state,
      label: t(stateKey[x.state]),
    }),
  );
  const [editing, setEditing] = useState<string | null>(null);
  // A benchmark's choice the edit page opens with.
  const [preset, setPreset] = useState<BenchChoice | null>(null);
  const [act, setAct] = useState<{ name: string; action: Action } | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const openTunnel = tunnels.find((x) => x.name === open) ?? null;
  const count = (s: TunnelState) => tunnels.filter((x) => x.state === s).length;
  const maxRate = Math.max(1, ...tunnels.map((x) => x.rate));

  // A tunnel that was deleted (here or elsewhere) takes the page back to the list.
  useEffect(() => {
    if (open && loaded && !openTunnel && !act) setOpen(null);
  }, [open, openTunnel, loaded, act]); // eslint-disable-line react-hooks/exhaustive-deps

  const q = query.trim().toLowerCase();
  const rows = tunnels.filter(
    (x) =>
      (filter === "all" || x.state === filter) &&
      (!q ||
        [x.name, x.label, x.entry?.server.name, x.exit?.server.name, x.transport, ...(x.entry?.tunnel.forwards ?? []).map((f) => f.listen)]
          .join(" ")
          .toLowerCase()
          .includes(q)),
  );

  const dialogs = (
    <>
      {act && (
        <RunDialog
          names={new Map(servers.map((x) => [x.id, x.name]))}
          key={`${act.name}-${act.action}`}
          title={t(act.action === "delete" ? "tun.deleteTitle" : act.action === "rotate" ? "tun.rotateTitle" : `tun.${act.action}`, { name: act.name })}
          text={act.action === "delete" ? t("tun.deleteText") : act.action === "rotate" ? t("tun.rotateText") : t(`tun.${act.action}Text`, { name: act.name })}
          danger={act.action === "delete"}
          confirm={act.action === "delete" ? act.name : undefined}
          autoClose={act.action === "delete"}
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
          onClose={() => {
            setWizard(false);
            onChanged();
          }}
        />
      )}
      {renaming && tunnels.find((x) => x.name === renaming) && (
        <RenameDialog
          tunnel={tunnels.find((x) => x.name === renaming)!}
          onClose={() => setRenaming(null)}
          onSaved={() => {
            setRenaming(null);
            onChanged();
          }}
        />
      )}
    </>
  );

  if (openTunnel && editing === openTunnel.name) {
    return (
      <TunnelEdit
        key={openTunnel.name}
        tunnel={openTunnel}
        servers={servers}
        preset={preset}
        onBack={() => {
          setEditing(null);
          setPreset(null);
          onChanged();
        }}
      />
    );
  }

  if (openTunnel) {
    return (
      <>
        <TunnelPage
          key={openTunnel.name}
          tunnel={openTunnel}
          onBack={() => setOpen(null)}
          servers={servers}
          onAct={(a) => setAct({ name: openTunnel.name, action: a })}
          onEdit={(choice) => {
            setPreset(choice ?? null);
            setEditing(openTunnel.name);
          }}
          onRename={() => setRenaming(openTunnel.name)}
        />
        {dialogs}
      </>
    );
  }

  return (
    <div className="page is-on">
      <div className="page-band">
        {order ? (
          <>
            <div className="grow" />
            <button className="btn btn-ghost btn-sm" type="button" onClick={() => setOrder(null)} disabled={saving}>
              {t("cancel")}
            </button>
            <button className="btn btn-primary btn-sm" type="button" onClick={saveOrder} disabled={saving}>
              <Icon name="check" size={18} />
              {t("arr.done")}
            </button>
          </>
        ) : (
          <>
            <Seg
              value={filter}
              options={(["all", "up", "down", "off"] as Filter[]).map((f) => [f, f === "all" ? t("tl.all") : t(stateKey[f]), num(f === "all" ? tunnels.length : count(f))] as [Filter, string, string])}
              onChange={setFilter}
            />
            <label className="search">
              <Icon name="search" size={18} />
              <input type="search" placeholder={t("tl.search")} aria-label={t("tl.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
            </label>
            <div className="grow" />
            {loaded && tunnels.length > 1 && (
              <button className="btn btn-ghost btn-sm" type="button" onClick={() => setOrder(tunnels.map((x) => x.name))}>
                <Icon name="arrange" size={18} />
                {t("arr.start")}
              </button>
            )}
            <button className="btn btn-primary btn-sm" type="button" disabled={online.length < 2} title={online.length < 2 ? t("tun.needTwo") : undefined} onClick={() => setWizard(true)}>
              <span className="shine" />
              <Icon name="plus" size={18} />
              {t("tun.new")}
            </button>
          </>
        )}
      </div>
      {order && loaded ? (
        <ArrangeList items={arrangeItems} order={order} onOrder={setOrder} hint={t("arr.hintTunnels")} />
      ) : !loaded ? (
        <div className="card">
          <Skeleton rows={4} height={44} />
        </div>
      ) : tunnels.length === 0 ? (
        <div className="card">
          <Empty
            icon="tunnels"
            title={t("t.emptyTitle")}
            text={online.length < 2 ? t("tun.needTwo") : t("tun.empty")}
            action={
              online.length >= 2 && (
                <button className="btn btn-primary btn-sm" type="button" onClick={() => setWizard(true)}>
                  <Icon name="plus" size={18} />
                  {t("tun.new")}
                </button>
              )
            }
          />
        </div>
      ) : (
        <div className="card flush">
          <div className="tunnels-wrap">
            <table className="tunnels">
              <thead>
                <tr>
                  <th>{t("t.name")}</th>
                  <th>{t("t.transport")}</th>
                  <th className="hide-sm">{t("t.ports")}</th>
                  <th>{t("t.state")}</th>
                  <th className="hide-sm">{t("t.uptime")}</th>
                  <th>{t("t.rate")}</th>
                  <th className="hide-sm">{t("t.month")}</th>
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
                    <tr key={x.name} className={`clickable st-${x.state}`} onClick={() => setOpen(x.name)}>
                      <td>
                        <div className="t-cell">
                          <span className={`t-mark ${x.state}`} aria-hidden="true" />
                          <div>
                            <button className={`linklike t-name ${x.label ? "is-label" : ""}`} type="button" dir="auto" onClick={() => setOpen(x.name)}>
                              {x.title}
                            </button>
                            {x.label && (
                              <span className="t-sys mono" dir="ltr">
                                {x.name}
                              </span>
                            )}
                            <span className="t-route">
                              {x.paired ? (
                                <>
                                  <span>{x.entry!.server.name}</span>
                                  <Icon name="arrow" size={14} />
                                  <span>{x.exit!.server.name}</span>
                                </>
                              ) : (
                                <span>
                                  {(x.entry ?? x.exit)!.server.name} · {t("t.oneSide")}
                                </span>
                              )}
                            </span>
                          </div>
                        </div>
                      </td>
                      <td className="c-transport">
                        {/* One quiet line; the profile and the rest are on the tunnel's own page. */}
                        <span className="t-spec">
                          <span className={`mono ${x.via ? "via" : ""}`} dir="ltr">
                            {transportLabel(x.transport, x.via)}
                          </span>
                          {mode && (
                            <>
                              <Sep />
                              <span>{t(`td.mode.${mode}`)}</span>
                            </>
                          )}
                        </span>
                      </td>
                      <td className="c-transport num hide-sm">{num((x.entry ?? x.exit)?.tunnel.forwards.length ?? 0)}</td>
                      <td>
                        <StateBadge state={x.state} />
                      </td>
                      <td className="hide-sm t-up">{x.uptime != null ? span(x.uptime) : "—"}</td>
                      <td className="rate num">
                        {x.state === "up" ? (
                          <div className="rate-cell">
                            <span dir="ltr">
                              {num(r.value, r.decimals)} <small>{r.unit}</small>
                            </span>
                            <span className="t-bar" aria-hidden="true">
                              <i style={{ width: `${Math.max(2, (x.rate / maxRate) * 100)}%` }} />
                            </span>
                          </div>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="num hide-sm" dir="ltr">
                        {(() => {
                          const m = usage.summary?.tunnels[x.name]?.marks[2];
                          if (!m) return "—";
                          const v = volume(m.rx + m.tx);
                          return (
                            <>
                              {num(v.value, v.decimals)} <small>{v.unit}</small>
                            </>
                          );
                        })()}
                      </td>
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
          </div>
        </div>
      )}
      {dialogs}
    </div>
  );
}
