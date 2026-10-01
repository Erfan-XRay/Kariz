import { useEffect, useMemo, useState } from "react";
import { ApiError, api } from "./api";
import type { PairRequest } from "./api";
import type { ServerInfo } from "./api";
import type { Tunnel } from "./derive";
import { useApp } from "./store";
import { Icon, Seg } from "./ui";
import { Checklist, opError, useOp } from "./ops";
import { MuxFields, MuxToggle, muxFromSpec, muxSpecOf, muxToSpec } from "./MuxFields";
import { TlsChoice, emptyTls, tlsReady } from "./TlsChoice";
import { MUX_OPTIONAL, TRANSPORTS, joinTransport, splitTransport } from "./transport";
import { PROFILES, addressesOf, hostOf, parsePorts, portOf } from "./Wizard";

/** Editing a tunnel: everything about it on one page, no steps. */
export function TunnelEdit({ tunnel, servers, onBack }: { tunnel: Tunnel; servers: ServerInfo[]; onBack: () => void }) {
  const { t } = useApp();
  const entry = tunnel.entry?.server.id ?? "";
  const exit = tunnel.exit?.server.id ?? "";
  const [loading, setLoading] = useState(true);
  // The tunnel cannot be edited now (a server is away, or it has one side only): say why.
  const [blocked, setBlocked] = useState(false);
  const [mode, setMode] = useState("reverse");
  const [transport, setTransport] = useState("tcp");
  const [muxOn, setMuxOn] = useState(true);
  const [profile, setProfile] = useState("balanced");
  const [listenHost, setListenHost] = useState("0.0.0.0");
  const [listenPort, setListenPort] = useState("");
  const [dialHost, setDialHost] = useState("");
  const [wsPath, setWsPath] = useState("/");
  const [wsHost, setWsHost] = useState("");
  const [sni, setSni] = useState("");
  const [pool, setPool] = useState<number | undefined>();
  const [ports, setPorts] = useState("");
  const [protocol, setProtocol] = useState("tcp");
  const [target, setTarget] = useState("127.0.0.1");
  const [mux, setMux] = useState(muxFromSpec());
  const [tls, setTls] = useState(emptyTls());
  const [rotate, setRotate] = useState(false);
  const [error, setError] = useState("");
  const [opId, setOpId] = useState<string | null>(null);
  const [initial, setInitial] = useState("");
  const op = useOp(opId);
  const running = !!opId && (!op || op.state === "running");

  const serverName = (id: string) => servers.find((s) => s.id === id)?.name ?? id;
  const acceptor = mode === "reverse" ? entry : exit;
  const dialer = mode === "reverse" ? exit : entry;
  const parsed = useMemo(() => parsePorts(ports, protocol, target.trim() || "127.0.0.1"), [ports, protocol, target]);
  const isWs = transport === "ws" || transport === "wss";
  const choices = (() => {
    const s = servers.find((x) => x.id === acceptor);
    return s ? addressesOf(s) : [];
  })();

  // What the two servers hold now.
  useEffect(() => {
    if (!tunnel.entry || !tunnel.exit) {
      setError(t("wz.oneSide"));
      setBlocked(true);
      setLoading(false);
      return;
    }
    const away = [entry, exit].filter((id) => !servers.find((s) => s.id === id)?.online);
    if (away.length) {
      setError(t("te.offline", { servers: away.map(serverName).join(", ") }));
      setBlocked(true);
      setLoading(false);
      return;
    }
    let alive = true;
    void Promise.all([api.tunnelSpec(entry, tunnel.name), api.tunnelSpec(exit, tunnel.name)])
      .then(([a, b]) => {
        if (!alive) return;
        const acc = a.mode === "reverse" ? a : b;
        const dia = a.mode === "reverse" ? b : a;
        setMode(a.mode);
        const split = splitTransport(a.transport, a.mux ?? b.mux);
        setTransport(split.transport);
        setMuxOn(split.mux);
        setProfile(a.profile ?? "balanced");
        setListenHost(hostOf(acc.listen) || "0.0.0.0");
        setListenPort(portOf(acc.listen));
        setDialHost(hostOf(dia.remote));
        setWsPath(a.ws_path ?? "/");
        setWsHost(a.ws_host ?? b.ws_host ?? "");
        setSni(dia.tls_sni ?? "");
        setPool(a.pool);
        const list = a.forwards;
        setPorts(list.map((f) => (portOf(f.listen) === portOf(f.target) ? portOf(f.listen) : `${portOf(f.listen)}=${portOf(f.target)}`)).join(", "));
        setProtocol(list[0]?.protocol ?? "tcp");
        setTarget(hostOf(list[0]?.target) || "127.0.0.1");
        setMux(muxFromSpec(a.mux ?? b.mux));
        const real = acc.tls_cert && acc.tls_key ? { cert: acc.tls_cert, key: acc.tls_key } : null;
        setTls(real ? { mode: "real", host: hostOf(dia.remote), email: "", ...real } : emptyTls());
        setInitial("");
      })
      .catch(() => {
        setError(t("tun.failed", { why: "no_such_tunnel" }));
        setBlocked(true);
      })
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [tunnel.name]); // eslint-disable-line react-hooks/exhaustive-deps

  const muxSpec = muxToSpec(mux);
  const request = (): PairRequest => ({
    name: tunnel.name,
    entry,
    exit,
    mode,
    transport: joinTransport(transport, muxOn).core,
    profile,
    listen: `${listenHost}:${listenPort}`,
    dial: `${dialHost.includes(":") && !dialHost.startsWith("[") ? `[${dialHost}]` : dialHost}:${listenPort}`,
    pool,
    ws_path: isWs ? wsPath : undefined,
    ws_host: isWs && wsHost.trim() ? wsHost.trim() : undefined,
    tls_sni: transport === "wss" && sni.trim() ? sni.trim() : undefined,
    mux: muxSpecOf(mux, transport, muxOn),
    tls_cert: transport === "wss" && tls.mode === "real" && tls.cert ? tls.cert : undefined,
    tls_key: transport === "wss" && tls.mode === "real" && tls.key ? tls.key : undefined,
    rotate: rotate ? true : undefined,
    forwards: parsed.forwards,
  });

  // What was loaded, to tell whether anything changed.
  const now = loading ? "" : JSON.stringify(request());
  useEffect(() => {
    if (!loading && !initial) setInitial(now);
  }, [loading, now]); // eslint-disable-line react-hooks/exhaustive-deps
  const dirty = !!initial && now !== initial;

  const problem = (): string => {
    if (!/^\d{1,5}$/.test(listenPort) || +listenPort < 1 || +listenPort > 65535) return t("wz.port", { server: serverName(acceptor) });
    if (!dialHost.trim()) return t("wz.dial", { server: serverName(acceptor), other: serverName(dialer) });
    if (isWs && !wsPath.startsWith("/")) return t("wz.wsPath");
    if (transport === "wss" && !tlsReady(tls)) return t("tls.needCert");
    if (parsed.bad !== undefined) return parsed.bad ? t("wz.badPorts", { bit: parsed.bad }) : t("wz.ports");
    if (muxSpec.bad) return `${t(`mux.${muxSpec.bad}`)}: ${t("mux.badValue")}`;
    return "";
  };

  const save = async () => {
    const p = problem();
    if (p) return setError(p);
    setError("");
    try {
      setOpId((await api.editTunnel(request())).op);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "busy" ? t("wz.busy") : e instanceof ApiError ? opError(t, e.code, new Map(servers.map((x) => [x.id, x.name]))) : String(e));
    }
  };

  const names = new Map(servers.map((x) => [x.id, x.name]));
  const choose = (list: readonly string[], value: string, set: (v: string) => void, text: (v: string) => [string, string]) => (
    <div className="tiles" role="radiogroup">
      {list.map((x) => {
        const [title, hint] = text(x);
        return (
          <button key={x} type="button" role="radio" aria-checked={value === x} className="tile" onClick={() => set(x)}>
            <span className="t1">{title}</span>
            <span className="t2">{hint}</span>
            <span className="tick">
              <Icon name="check" size={14} />
            </span>
          </button>
        );
      })}
    </div>
  );

  return (
    <div className="page is-on tunnel-edit">
      <section className="card edit-head">
        <button className="icon-square td-back" type="button" onClick={onBack} disabled={running} aria-label={t("td.back")} title={t("td.back")}>
          <Icon name="back" size={18} />
        </button>
        <div className="td-title">
          <h2 className="td-name mono">{tunnel.name}</h2>
          <p className="muted small">{t("te.lead", { entry: serverName(entry), exit: serverName(exit) })}</p>
        </div>
      </section>

      {blocked ? (
        <section className="card edit-sec">
          <p className="err small" role="alert">
            {error}
          </p>
          <div className="edit-bar">
            <button className="btn btn-ghost" type="button" onClick={onBack}>
              {t("td.back")}
            </button>
          </div>
        </section>
      ) : loading ? (
        <section className="card">
          <p className="muted">…</p>
        </section>
      ) : (
        <>
          <fieldset className="card edit-sec" disabled={running || op?.state === "done"}>
            <legend>{t("te.how")}</legend>
            <div className="field">
              <span className="label">{t("wz.q2")}</span>
              <Seg value={mode} options={[["reverse", t("wz.mode.reverse")], ["direct", t("wz.mode.direct")]]} onChange={setMode} />
              <span className="help">{t(`wz.mode.${mode}.d`)}</span>
            </div>
            <div className="field">
              <span className="label">{t("wz.transport")}</span>
              {choose(TRANSPORTS, transport, setTransport, (x) => [x, t(`wz.tr.${x}`)])}
              {joinTransport(transport, muxOn).core !== tunnel.transport && <span className="help warn-text">{t("te.transportChange")}</span>}
            </div>
            <div className="field">
              <span className="label">{t("wz.profile")}</span>
              {choose(PROFILES, profile, setProfile, (x) => [t(`wz.pf.${x}`), t(`wz.pf.${x}.d`)])}
            </div>
          </fieldset>

          <fieldset className="card edit-sec" disabled={running || op?.state === "done"}>
            <legend>{t("te.where")}</legend>
            <p className="muted small">{t("wz.lead3", { acceptor: serverName(acceptor), dialer: serverName(dialer) })}</p>
            <div className="grid-2">
              <div className="field">
                <label htmlFor="te-port">{t("wz.port", { server: serverName(acceptor) })}</label>
                <input className="text mono" id="te-port" dir="ltr" inputMode="numeric" value={listenPort} onChange={(e) => setListenPort(e.target.value.trim())} />
                {transport === "auto" && <span className="help">{t("wz.autoPorts", { server: serverName(acceptor), tcp: listenPort, udp: listenPort, ws: String(+listenPort + 1) })}</span>}
              </div>
              <div className="field">
                <label htmlFor="te-dial">{t("wz.dial", { server: serverName(acceptor), other: serverName(dialer) })}</label>
                <input className="text mono" id="te-dial" dir="ltr" value={dialHost} placeholder="203.0.113.5" onChange={(e) => setDialHost(e.target.value.trim())} />
                {choices.length > 0 && (
                  <span className="addr-chips">
                    {choices.map((a) => (
                      <button key={a} type="button" className={`pchip as-btn ${a === dialHost ? "on" : ""}`} dir="ltr" onClick={() => setDialHost(a)}>
                        {a}
                      </button>
                    ))}
                  </span>
                )}
                {mode === "direct" && <span className="help">{t("wz.directHelp", { server: serverName(acceptor), port: listenPort })}</span>}
              </div>
              {isWs && (
                <div className="field">
                  <label htmlFor="te-ws">{t("wz.wsPath")}</label>
                  <input className="text mono" id="te-ws" dir="ltr" value={wsPath} onChange={(e) => setWsPath(e.target.value.trim())} />
                </div>
              )}
              {isWs && (
                <div className="field">
                  <label htmlFor="te-wshost">{t("wz.wsHost")}</label>
                  <input className="text mono" id="te-wshost" dir="ltr" value={wsHost} placeholder="cdn.example.com" onChange={(e) => setWsHost(e.target.value.trim())} />
                  <span className="help">{t("wz.wsHostHelp")}</span>
                </div>
              )}
              {transport === "wss" && (
                <div style={{ gridColumn: "1 / -1" }}>
                  <TlsChoice value={tls} onChange={setTls} server={acceptor} serverName={serverName(acceptor)} onHost={(h) => setDialHost(h)} />
                </div>
              )}
              {transport === "wss" && tls.mode === "self" && (
                <div className="field">
                  <label htmlFor="te-sni">{t("wz.sni")}</label>
                  <input className="text mono" id="te-sni" dir="ltr" value={sni} placeholder="www.example.com" onChange={(e) => setSni(e.target.value.trim())} />
                  <span className="help">{t("wz.sniHelp")}</span>
                </div>
              )}
              {transport === "tcp" && !muxOn && (
                <div className="field">
                  <label htmlFor="te-pool">{t("wz.pool")}</label>
                  <input
                    className="text mono"
                    id="te-pool"
                    dir="ltr"
                    inputMode="numeric"
                    value={pool ?? ""}
                    placeholder="8"
                    onChange={(e) => setPool(/^\d{1,3}$/.test(e.target.value.trim()) ? +e.target.value.trim() : undefined)}
                  />
                  <span className="help">{t("wz.poolHelp")}</span>
                </div>
              )}
            </div>
          </fieldset>

          <fieldset className="card edit-sec" disabled={running || op?.state === "done"}>
            <legend>{t("te.ports")}</legend>
            <p className="muted small">{t("wz.lead4")}</p>
            <div className="field" style={{ maxWidth: 520 }}>
              <label htmlFor="te-ports">{t("wz.ports")}</label>
              <input className="text mono" id="te-ports" dir="ltr" value={ports} placeholder="443, 8080-8090" onChange={(e) => setPorts(e.target.value)} />
              <span className="help">{t("wz.portsHelp")}</span>
            </div>
            <div className="port-chips" aria-live="polite">
              {parsed.forwards.slice(0, 40).map((f) => (
                <span className="pchip" key={f.listen + f.target}>
                  {f.listen.split(":").pop()}
                  <small>→ {f.target.split(":").pop()}</small>
                </span>
              ))}
              {parsed.forwards.length > 40 && <span className="pchip">+{parsed.forwards.length - 40}</span>}
              {parsed.bad && <span className="pchip bad">{parsed.bad}</span>}
            </div>
            <div className="grid-2" style={{ maxWidth: 520 }}>
              <div className="field">
                <span className="label">{t("wz.protocol")}</span>
                <Seg value={protocol} options={[["tcp", "TCP"], ["udp", "UDP"], ["tcp+udp", "TCP+UDP"]]} onChange={setProtocol} />
              </div>
              <div className="field">
                <label htmlFor="te-target">{t("wz.targetHost")}</label>
                <input className="text mono" id="te-target" dir="ltr" value={target} onChange={(e) => setTarget(e.target.value.trim())} />
              </div>
            </div>
          </fieldset>

          <fieldset className="card edit-sec" disabled={running || op?.state === "done"}>
            <legend>{t("mux.title")}</legend>
            <MuxToggle transport={transport} value={muxOn} onChange={setMuxOn} />
            {(muxOn || !MUX_OPTIONAL.includes(transport)) && <MuxFields value={mux} onChange={setMux} profile={profile} transport={transport} />}
          </fieldset>

          <fieldset className="card edit-sec" disabled={running || op?.state === "done"}>
            <legend>{t("te.security")}</legend>
            <label className="check">
              <input type="checkbox" checked={rotate} onChange={(e) => setRotate(e.target.checked)} /> {t("wz.rotate")}
            </label>
          </fieldset>

          {(opId || error) && (
            <section className="card edit-sec" aria-live="polite">
              {error && (
                <p className="err small" role="alert">
                  {error}
                </p>
              )}
              <Checklist op={op} />
              {op?.state === "done" && <p className="ok small">{t("tun.done")}</p>}
              {op?.state === "failed" && (
                <>
                  <p className="err small">{t("tun.failed", { why: opError(t, op.error, names) })}</p>
                  <p className="muted small">{op.undone ? t("tun.undone") : t("tun.notUndone")}</p>
                </>
              )}
            </section>
          )}

          <div className="edit-bar">
            <button className="btn btn-ghost" type="button" disabled={running} onClick={onBack}>
              {op?.state === "done" ? t("wz.close") : t("cancel")}
            </button>
            <span className="grow" />
            {op?.state === "failed" && (
              <button className="btn btn-ghost" type="button" onClick={() => setOpId(null)}>
                {t("wz.retry")}
              </button>
            )}
            {!opId && (
              <button className="btn btn-primary" type="button" disabled={!dirty} onClick={() => void save()}>
                <span className="shine" />
                {t("wz.save")}
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
