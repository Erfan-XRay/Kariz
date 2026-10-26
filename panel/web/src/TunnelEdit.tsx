import { useEffect, useMemo, useState } from "react";
import { ApiError, api } from "./api";
import type { PairRequest, Spec, TunnelInfo } from "./api";
import type { ServerInfo } from "./api";
import type { Tunnel } from "./derive";
import { useApp } from "./store";
import { Icon, Seg } from "./ui";
import { Checklist, opError, useOp } from "./ops";
import { MuxFields, MuxToggle, muxFromSpec, muxSpecOf, muxToSpec } from "./MuxFields";
import { TlsChoice, emptyTls, isLetsEncrypt, tlsReady } from "./TlsChoice";
import { EncryptionChoice, cipherOf, cipherReady } from "./EncryptionChoice";
import { QuicObfs, obfsOf } from "./QuicObfs";
import { PerIpLimit, perIpOf } from "./PerIpLimit";
import { UdpFlows, udpFlowsOf } from "./UdpFlows";
import { ProxyProtocol, proxyFromSpec, proxyOf } from "./ProxyProtocol";
import type { ProxyChoice } from "./ProxyProtocol";
import { GreChoice, linkWithAddress } from "./GreChoice";
import { useNetworks } from "./Networks";
import { MUX_OPTIONAL, TRANSPORTS, joinTransport, splitTransport } from "./transport";
import { PROFILES, addressesOf, hostOf, parsePorts, portOf } from "./Wizard";
import { Choice, More, noteOf, pairOf } from "./Choice";
import { benchLabel, isV6 } from "./Benchmark";
import type { BenchChoice } from "./Benchmark";

/** What the panel last saw of a side it cannot ask now (its server is offline), as a spec to start from. */
function specOf(name: string, info: TunnelInfo): Spec {
  return {
    name,
    role: info.role,
    mode: info.mode,
    transport: info.transport,
    profile: info.profile,
    listen: info.listen ?? undefined,
    remote: info.remote ?? undefined,
    encryption: info.encryption || undefined,
    forwards: info.forwards,
  };
}

/** Editing a tunnel: everything about it on one page, no steps. */
export function TunnelEdit({ tunnel, servers, preset, onBack }: { tunnel: Tunnel; servers: ServerInfo[]; preset?: BenchChoice | null; onBack: () => void }) {
  const { t } = useApp();
  // The servers it is on now, and the ones it is to be on (another server moves that side there).
  const wasEntry = tunnel.entry?.server.id ?? "";
  const wasExit = tunnel.exit?.server.id ?? "";
  const [entry, setEntry] = useState(wasEntry);
  const [exit, setExit] = useState(wasExit);
  const online = (id: string) => !!servers.find((s) => s.id === id)?.online;
  // A side whose server is offline was read from what the panel last saw of it.
  const [away, setAway] = useState<string[]>([]);
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
  const [enc, setEnc] = useState("auto");
  const [encAck, setEncAck] = useState(false);
  const [obfs, setObfs] = useState(false);
  const [perIp, setPerIp] = useState("");
  const [udpFlows, setUdpFlows] = useState("");
  const [proxy, setProxy] = useState<ProxyChoice>("off");
  // The private GRE network the tunnel runs over (its id), or none: found from the address the
  // tunnel listens on once the networks have come in.
  const [netId, setNetId] = useState("");
  const [loadedHost, setLoadedHost] = useState<string | null>(null);
  const [netReady, setNetReady] = useState(false);
  const { networks, links, loaded: netsLoaded } = useNetworks();
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
    // One side offline: the tunnel can still be edited, to move that side to a server that is there.
    const off = [wasEntry, wasExit].filter((id) => !online(id));
    if (off.length === 2) {
      setError(t("te.offline", { servers: off.map(serverName).join(", ") }));
      setBlocked(true);
      setLoading(false);
      return;
    }
    setAway(off);
    let alive = true;
    const read = (side: typeof tunnel.entry, id: string) => (online(id) || !side ? api.tunnelSpec(id, tunnel.name) : Promise.resolve(specOf(tunnel.name, side.tunnel)));
    void Promise.all([read(tunnel.entry, wasEntry), read(tunnel.exit, wasExit)])
      .then(([a, b]) => {
        if (!alive) return;
        const acc = a.mode === "reverse" ? a : b;
        const dia = a.mode === "reverse" ? b : a;
        setMode(a.mode);
        const split = splitTransport(a.transport || b.transport, a.mux ?? b.mux);
        setTransport(split.transport);
        setMuxOn(split.mux);
        setProfile(a.profile ?? "balanced");
        setLoadedHost(hostOf(acc.listen));
        setListenHost(hostOf(acc.listen) || "0.0.0.0");
        setListenPort(portOf(acc.listen));
        setDialHost(hostOf(dia.remote));
        setWsPath(a.ws_path ?? b.ws_path ?? "/");
        setWsHost(a.ws_host ?? b.ws_host ?? "");
        const real = acc.tls_cert && acc.tls_key ? { cert: acc.tls_cert, key: acc.tls_key } : null;
        // With a real certificate the server name is the certificate's own (not one to edit).
        setSni(real ? "" : (dia.tls_sni ?? ""));
        setPool(a.pool ?? b.pool);
        const list = a.forwards;
        setPorts(list.map((f) => (portOf(f.listen) === portOf(f.target) ? portOf(f.listen) : `${portOf(f.listen)}=${portOf(f.target)}`)).join(", "));
        setProtocol(list[0]?.protocol ?? "tcp");
        setTarget(hostOf(list[0]?.target) || "127.0.0.1");
        setMux(muxFromSpec(a.mux ?? b.mux));
        setEnc(a.encryption ?? "auto");
        setObfs(!!(a.quic_obfs ?? b.quic_obfs));
        setPerIp(String(a.max_conns_per_ip ?? b.max_conns_per_ip ?? ""));
        setUdpFlows(String(a.udp_max_flows ?? b.udp_max_flows ?? ""));
        setProxy(proxyFromSpec(a.proxy_protocol ?? b.proxy_protocol));
        // A tunnel that has no encryption already was confirmed when it was made.
        setEncAck((a.encryption ?? "auto") === "none");
        setTls(real ? { mode: isLetsEncrypt(real.cert) ? "real" : "own", host: dia.tls_sni || hostOf(dia.remote), email: "", ...real } : emptyTls());
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

  // A tunnel that listens on the private address of a link runs over that link's network.
  useEffect(() => {
    if (loadedHost === null || !netsLoaded || netReady) return;
    const link = linkWithAddress(links, loadedHost);
    if (link) setNetId(link.network);
    setNetReady(true);
  }, [loadedHost, netsLoaded, netReady, links]);

  // Off the network, the tunnel goes back to the public addresses: it listens on every address and
  // dials the one the panel knows the accepting server by (changeable below).
  const chooseNetwork = (id: string) => {
    if (!id && netId) {
      setListenHost("0.0.0.0");
      setDialHost(choices[0] ?? "");
    }
    setNetId(id);
  };

  // Another server for a side: the tunnel moves there (the other side follows it). The accepting
  // server's address and certificate were that server's, so they start again for the new one.
  const moved = entry !== wasEntry || exit !== wasExit;
  const wasAcceptor = mode === "reverse" ? wasEntry : wasExit;
  const [certMoved, setCertMoved] = useState(false);
  const pick = (side: "entry" | "exit", id: string) => {
    const next = side === "entry" ? { entry: id, exit } : { entry, exit: id };
    const nextAcceptor = mode === "reverse" ? next.entry : next.exit;
    if (nextAcceptor !== acceptor) refit(nextAcceptor);
    setEntry(next.entry);
    setExit(next.exit);
  };
  const swap = () => {
    refit(dialer);
    setEntry(exit);
    setExit(entry);
  };
  const refit = (to: string) => {
    const s = servers.find((x) => x.id === to);
    if (!netId) setDialHost(s ? (addressesOf(s)[0] ?? "") : "");
    if (!["0.0.0.0", "[::]"].includes(listenHost)) setListenHost("0.0.0.0");
    if (tls.mode !== "self") {
      setTls(emptyTls());
      setCertMoved(true);
    }
  };
  // The servers the tunnel leaves (none when the two only swap).
  const left = [wasEntry, wasExit].filter((id) => id && id !== entry && id !== exit);
  // A side left on an offline server cannot be saved: it has to move.
  const stuck = away.filter((id) => id === entry || id === exit);

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
    network: netId || undefined,
    pool,
    ws_path: isWs ? wsPath : undefined,
    ws_host: isWs && wsHost.trim() ? wsHost.trim() : undefined,
    tls_sni: transport === "wss" && tls.mode === "self" && sni.trim() ? sni.trim() : undefined,
    mux: muxSpecOf(mux, transport, muxOn),
    encryption: cipherOf(enc, transport),
    quic_obfs: obfsOf(obfs, transport),
    max_conns_per_ip: perIpOf(perIp).value,
    udp_max_flows: protocol.includes("udp") ? udpFlowsOf(udpFlows).value : undefined,
    proxy_protocol: proxyOf(proxy, protocol),
    tls_cert: transport === "wss" && tls.mode !== "self" && tls.cert ? tls.cert : undefined,
    tls_key: transport === "wss" && tls.mode !== "self" && tls.key ? tls.key : undefined,
    tls_host: transport === "wss" && tls.mode !== "self" && tls.cert && tls.host ? tls.host.toLowerCase() : undefined,
    rotate: rotate ? true : undefined,
    forwards: parsed.forwards,
  });

  // What was loaded, to tell whether anything changed.
  const ready = !loading && (blocked || netReady);
  const now = ready ? JSON.stringify(request()) : "";
  useEffect(() => {
    if (ready && !initial) setInitial(now);
  }, [ready, now]); // eslint-disable-line react-hooks/exhaustive-deps
  const dirty = !!initial && now !== initial;

  // Dialed at an IPv6 address, the accepting side listens on every address, IPv6 too (on every IPv4 one it
  // would never answer).
  useEffect(() => {
    if (!netId && isV6(dialHost) && listenHost === "0.0.0.0") setListenHost("[::]");
  }, [dialHost, netId, listenHost]);

  // Opened from a benchmark result: once what the tunnel has is known, its direction, transport, network, the
  // IPv6 address of an IPv6 result and the server name of a wss one are put in, as changes to look over and save.
  const [presetDone, setPresetDone] = useState(false);
  useEffect(() => {
    if (!preset || presetDone || !initial || blocked) return;
    setMode(preset.mode);
    setTransport(preset.transport === "tcpmux" ? "tcp" : preset.transport);
    setMuxOn(true);
    chooseNetwork(preset.network ?? "");
    if (preset.host && (preset.path === "ipv6" || isV6(dialHost))) setDialHost(preset.host);
    if (preset.sni) setSni(preset.sni);
    setPresetDone(true);
  }, [initial]); // eslint-disable-line react-hooks/exhaustive-deps

  const problem = (): string => {
    if (entry === exit) return t("te.sameServer");
    if (stuck.length) return t("te.awayStay", { server: serverName(stuck[0]) });
    if (moved && [entry, exit].some((id) => !online(id))) return t("te.moveOffline");
    if (!/^\d{1,5}$/.test(listenPort) || +listenPort < 1 || +listenPort > 65535) return t("wz.port", { server: serverName(acceptor) });
    if (!dialHost.trim() && !netId) return t("wz.dial", { server: serverName(acceptor), other: serverName(dialer) });
    if (isWs && !wsPath.startsWith("/")) return t("wz.wsPath");
    if (transport === "wss" && !tlsReady(tls)) return t(tls.mode === "own" ? "tls.needOwn" : "tls.needCert");
    if (!cipherReady(enc, transport, encAck)) return t("enc.needAck");
    if (parsed.bad !== undefined) return parsed.bad ? t("wz.badPorts", { bit: parsed.bad }) : t("wz.ports");
    if (muxSpec.bad) return `${t(`mux.${muxSpec.bad}`)}: ${t("mux.badValue")}`;
    if (perIpOf(perIp).bad) return t("perip.bad");
    if (protocol.includes("udp") && udpFlowsOf(udpFlows).bad) return t("udpflows.bad");
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
  const locked = running || op?.state === "done";
  // The folded card: what is set in it (or what it holds), and whether the error is about one of its fields.
  const udpPorts = protocol.includes("udp");
  const moreNote = noteOf(
    [
      transport !== "quic" && enc !== "auto" && t(`enc.${enc}`),
      transport === "quic" && obfs && "obfs",
      MUX_OPTIONAL.includes(transport) && !muxOn && pairOf("mux", t("mux.off")),
      !!muxSpec.spec && t("mux.title"),
      isWs && wsPath !== "/" && pairOf(t("wz.wsPath"), wsPath),
      isWs && wsHost && pairOf("Host", wsHost),
      transport === "wss" && tls.mode === "self" && sni && pairOf("SNI", sni),
      transport === "tcp" && !muxOn && pool !== undefined && pairOf(t("wz.pool"), pool),
      perIp && pairOf(t("perip.title"), perIp),
      udpPorts && udpFlows && pairOf(t("udpflows.title"), udpFlows),
      !!proxyOf(proxy, protocol) && pairOf(t("proxy.short"), proxy),
      rotate && t("te.rotateOn"),
    ],
    [t("enc.title"), "mux", t("perip.title"), protocol !== "udp" && t("proxy.short"), t("te.token")],
  );
  const moreBad =
    !cipherReady(enc, transport, encAck) ||
    !!muxSpec.bad ||
    !!perIpOf(perIp).bad ||
    (udpPorts && !!udpFlowsOf(udpFlows).bad) ||
    (isWs && !wsPath.startsWith("/"));

  return (
    <div className="page is-on tunnel-edit">
      <section className="card edit-head">
        <button className="icon-square td-back" type="button" onClick={onBack} disabled={running} aria-label={t("td.back")} title={t("td.back")}>
          <Icon name="back" size={18} />
        </button>
        <div className="td-title">
          <h2 className="td-name mono">{tunnel.name}</h2>
          <p className="muted small">{t("te.lead", { entry: serverName(wasEntry), exit: serverName(wasExit) })}</p>
        </div>
      </section>
      {preset && !blocked && !loading && (
        <div className="banner info">
          <Icon name="gauge" size={18} />
          <span>{t("te.fromBench", { t: benchLabel(preset.transport), mode: t(`bench.mode.${preset.mode}`) })}</span>
        </div>
      )}

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
          <fieldset className="card edit-sec" disabled={locked}>
            <legend>{t("te.servers")}</legend>
            <p className="muted small">{t("te.serversLead")}</p>
            <div className="te-servers">
              {(["entry", "exit"] as const).map((side) => {
                const value = side === "entry" ? entry : exit;
                const other = side === "entry" ? exit : entry;
                const was = side === "entry" ? wasEntry : wasExit;
                return (
                  <div className="field" key={side}>
                    <label htmlFor={`te-${side}`}>{t(side === "entry" ? "te.entry" : "te.exit")}</label>
                    <select className="select" id={`te-${side}`} value={value} onChange={(e) => pick(side, e.target.value)}>
                      {servers.map((s) => (
                        <option key={s.id} value={s.id} disabled={s.id === other || (!s.online && s.id !== was)}>
                          {s.name}
                          {!s.online ? ` (${t("te.offlineTag")})` : ""}
                        </option>
                      ))}
                    </select>
                    {value !== was && <span className="help">{t("te.from", { server: serverName(was) })}</span>}
                  </div>
                );
              })}
              <button className="btn btn-ghost btn-sm te-swap" type="button" onClick={swap} title={t("te.swap")}>
                <Icon name="arrange" size={16} />
                {t("te.swap")}
              </button>
            </div>
            {stuck.length > 0 && (
              <div className="banner warn">
                <Icon name="alert" size={18} />
                <span>{t("te.awayStay", { server: serverName(stuck[0]) })}</span>
              </div>
            )}
            {away.includes(wasEntry) && <p className="help">{t("te.fromInfo", { server: serverName(wasEntry) })}</p>}
            {moved && (
              <div className="banner info">
                <Icon name="info" size={18} />
                <span>
                  {t(left.length ? "te.moveText" : "te.swapText", {
                    from: left.map(serverName).join(", "),
                    entry: serverName(entry),
                    exit: serverName(exit),
                  })}
                  {away.some((id) => id !== entry && id !== exit) && ` ${t("te.moveAway")}`}
                </span>
              </div>
            )}
            {certMoved && wasAcceptor !== acceptor && <p className="help warn-text">{t("te.certMoved", { server: serverName(acceptor) })}</p>}
          </fieldset>

          <fieldset className="card edit-sec" disabled={locked}>
            <legend>{t("te.how")}</legend>
            <Choice label={t("wz.modeLabel")} value={mode} options={[["reverse", t("wz.mode.reverse")], ["direct", t("wz.mode.direct")]]} onChange={setMode} hint={t(`wz.mode.${mode}.d`)} />
            <Choice label={t("wz.transport")} mono value={transport} options={TRANSPORTS.map((x) => [x, x])} onChange={setTransport} hint={t(`wz.tr.${transport}`)}>
              {joinTransport(transport, muxOn).core !== tunnel.transport && <span className="help warn-text">{t("te.transportChange")}</span>}
            </Choice>
            <Choice label={t("wz.profile")} value={profile} options={PROFILES.map((x) => [x, t(`wz.pf.${x}`)])} onChange={setProfile} hint={t(`wz.pf.${profile}.d`)} />
          </fieldset>

          <fieldset className="card edit-sec" disabled={locked}>
            <legend>{t("te.where")}</legend>
            <p className="muted small">{t("wz.lead3", { acceptor: serverName(acceptor), dialer: serverName(dialer) })}</p>
            <GreChoice value={netId} onChange={chooseNetwork} networks={networks} links={links} entry={entry} exit={exit} acceptor={acceptor} port={listenPort} serverName={serverName} />
            {!netId && loadedHost !== null && !!linkWithAddress(links, loadedHost) && <p className="help warn-text">{t("te.greOff")}</p>}
            <div className="grid-2">
              <div className="field">
                <label htmlFor="te-port">{t("wz.port", { server: serverName(acceptor) })}</label>
                <input className="text mono" id="te-port" dir="ltr" inputMode="numeric" value={listenPort} onChange={(e) => setListenPort(e.target.value.trim())} />
                {transport === "auto" && <span className="help">{t("wz.autoPorts", { server: serverName(acceptor), tcp: listenPort, udp: listenPort, ws: String(+listenPort + 1) })}</span>}
                {(transport === "quic" || transport === "kcp") && <span className="help">{t("wz.portUdp")}</span>}
              </div>
              <div className="field" hidden={!!netId}>
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
            </div>
            {transport === "wss" && <TlsChoice value={tls} onChange={setTls} server={acceptor} serverName={serverName(acceptor)} onHost={(h) => setDialHost(h)} />}
          </fieldset>

          <fieldset className="card edit-sec" disabled={locked}>
            <legend>{t("te.ports")}</legend>
            <p className="muted small">{t("wz.lead4")}</p>
            <div className="field wz-narrow">
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
            <div className="grid-2 wz-narrow">
              <div className="field">
                <span className="label">{t("wz.protocol")}</span>
                <Seg value={protocol} options={[["tcp", "TCP"], ["udp", "UDP"], ["tcp+udp", "TCP+UDP"]]} onChange={setProtocol} label={t("wz.protocol")} />
              </div>
              <div className="field">
                <label htmlFor="te-target">{t("wz.targetHost")}</label>
                <input className="text mono" id="te-target" dir="ltr" value={target} onChange={(e) => setTarget(e.target.value.trim())} />
              </div>
            </div>
          </fieldset>

          {/* What most tunnels never change, folded into one card. */}
          <fieldset className="card edit-sec edit-more" disabled={locked}>
            <legend className="sr-only">{t("more.title")}</legend>
            <More title={t("more.title")} note={moreNote} show={!!error && moreBad}>
              <div className="more-group">
                <h4 className="more-h">{t("te.how")}</h4>
                <EncryptionChoice value={enc} onChange={setEnc} transport={transport} ack={encAck} onAck={setEncAck} />
                {transport === "quic" && <QuicObfs value={obfs} onChange={setObfs} />}
                <MuxToggle transport={transport} value={muxOn} onChange={setMuxOn} />
                {(muxOn || !MUX_OPTIONAL.includes(transport)) && (
                  <More title={t("mux.title")} note={muxSpec.spec ? t("mux.custom") : t("mux.fromProfile")} show={!!muxSpec.bad}>
                      <MuxFields value={mux} onChange={setMux} profile={profile} transport={transport} />
                    </More>
                )}
              </div>
              {(isWs || (transport === "tcp" && !muxOn)) && (
                <div className="more-group">
                  <h4 className="more-h">{t("te.where")}</h4>
                  <div className="grid-2">
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
                </div>
              )}
              <div className="more-group more-fields wz-narrow">
                <h4 className="more-h">{t("te.ports")}</h4>
                <PerIpLimit id="te-perip" value={perIp} onChange={setPerIp} />
                <UdpFlows id="te-udpflows" value={udpFlows} onChange={setUdpFlows} protocol={protocol} />
                <ProxyProtocol value={proxy} onChange={setProxy} protocol={protocol} />
              </div>
              <div className="more-group">
                <h4 className="more-h">{t("te.security")}</h4>
                <label className="check">
                  <input type="checkbox" checked={rotate || moved} disabled={moved} onChange={(e) => setRotate(e.target.checked)} /> {t("wz.rotate")}
                </label>
                {moved && <span className="help">{t("te.moveToken")}</span>}
              </div>
            </More>
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
              {op?.state === "done" && !!op.offline?.length && (
                <p className="warn-text small" role="status">
                  {t("te.movedLater", { servers: op.offline.map((id) => names.get(id) ?? id).join(", ") })}
                </p>
              )}
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
