import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ApiError, api } from "./api";
import type { CheckReply, ForwardSpec, PairRequest, ServerInfo } from "./api";
import type { Tunnel } from "./derive";
import { useApp } from "./store";
import { Icon, Seg, useFocusTrap } from "./ui";
import { Checklist, opError, useOp } from "./ops";
import { useNetworks } from "./Networks";

const TRANSPORTS = ["tcp", "tcpmux", "ws", "wss", "quic", "kcp"] as const;
const PROFILES = ["balanced", "ultraspeed", "gaming"] as const;
const NAME = /^[a-z0-9][a-z0-9-]{0,31}$/;

/** "443, 8080-8090, 2053=53" as forwards; the piece that is wrong if one is. */
export function parsePorts(text: string, protocol: string, host: string): { forwards: ForwardSpec[]; bad?: string } {
  const forwards: ForwardSpec[] = [];
  const port = (s: string) => (/^\d{1,5}$/.test(s) && +s >= 1 && +s <= 65535 ? +s : null);
  for (const raw of text.split(",")) {
    const bit = raw.trim();
    if (!bit) continue;
    const [left, right] = bit.split("=").map((x) => x.trim());
    const range = left.match(/^(\d+)-(\d+)$/);
    if (range) {
      const a = port(range[1]);
      const b = port(range[2]);
      if (a === null || b === null || b < a || right !== undefined || forwards.length + (b - a + 1) > 200) return { forwards, bad: bit };
      for (let p = a; p <= b; p++) forwards.push({ listen: `0.0.0.0:${p}`, target: `${host}:${p}`, protocol });
      continue;
    }
    const l = port(left);
    const r = right === undefined ? l : port(right);
    if (l === null || r === null) return { forwards, bad: bit };
    forwards.push({ listen: `0.0.0.0:${l}`, target: `${host}:${r}`, protocol });
  }
  if (forwards.length === 0) return { forwards, bad: "" };
  return { forwards };
}

/** The port part of an address like `0.0.0.0:3080`. */
const portOf = (addr: string | undefined) => addr?.split(":").pop() ?? "";
const hostOf = (addr: string | undefined) => (addr ? addr.slice(0, addr.lastIndexOf(":")) : "");

export function Wizard({ servers, edit, onClose }: { servers: ServerInfo[]; edit?: Tunnel; onClose: () => void }) {
  const { t, num, lang } = useApp();
  const editing = !!edit;
  const online = servers.filter((s) => s.online);
  const [on, setOn] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useFocusTrap(box);
  const [step, setStep] = useState(0);
  const [back, setBack] = useState(false);
  const [name, setName] = useState(edit?.name ?? "");
  const [entry, setEntry] = useState(edit?.entry?.server.id ?? "");
  const [exit, setExit] = useState(edit?.exit?.server.id ?? "");
  const [mode, setMode] = useState("reverse");
  const [transport, setTransport] = useState<string>("tcpmux");
  const [profile, setProfile] = useState<string>("balanced");
  const [listenPort, setListenPort] = useState("3080");
  const [dialHost, setDialHost] = useState("");
  // The host the accepting side listens on: every address, or (for a tunnel that was made
  // over a private network) its private one.
  const [listenHost, setListenHost] = useState("0.0.0.0");
  const [netId, setNetId] = useState("");
  const { networks, links } = useNetworks();
  const [wsPath, setWsPath] = useState("/");
  const [ports, setPorts] = useState("");
  const [protocol, setProtocol] = useState("tcp");
  const [target, setTarget] = useState("127.0.0.1");
  const [pool, setPool] = useState<number | undefined>();
  const [loading, setLoading] = useState(editing);
  const [check, setCheck] = useState<{ entry: CheckReply; exit: CheckReply } | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState("");
  const [opId, setOpId] = useState<string | null>(null);
  const op = useOp(opId);

  useEffect(() => {
    const id = requestAnimationFrame(() => setOn(true));
    return () => cancelAnimationFrame(id);
  }, []);

  // Editing: what the two servers hold now.
  useEffect(() => {
    if (!edit?.entry || !edit.exit) {
      // A tunnel seen from one side only cannot be edited as a pair: say so instead of waiting.
      if (edit) {
        setError(t("wz.oneSide"));
        setLoading(false);
      }
      return;
    }
    let alive = true;
    void Promise.all([api.tunnelSpec(edit.entry.server.id, edit.name), api.tunnelSpec(edit.exit.server.id, edit.name)])
      .then(([a, b]) => {
        if (!alive) return;
        const acceptor = a.mode === "reverse" ? a : b;
        const dialer = a.mode === "reverse" ? b : a;
        setMode(a.mode);
        setTransport(a.transport);
        setProfile(a.profile ?? "balanced");
        setListenPort(portOf(acceptor.listen));
        setListenHost(hostOf(acceptor.listen) || "0.0.0.0");
        setDialHost(hostOf(dialer.remote));
        setWsPath(a.ws_path ?? "/");
        setPool(a.pool);
        const list = a.forwards;
        setPorts(list.map((f) => (portOf(f.listen) === portOf(f.target) ? portOf(f.listen) : `${portOf(f.listen)}=${portOf(f.target)}`)).join(", "));
        setProtocol(list[0]?.protocol ?? "tcp");
        setTarget(hostOf(list[0]?.target) || "127.0.0.1");
      })
      .catch(() => setError(t("tun.failed", { why: "no_such_tunnel" })))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [edit]); // eslint-disable-line react-hooks/exhaustive-deps

  const serverName = (id: string) => servers.find((s) => s.id === id)?.name ?? id;
  const acceptor = mode === "reverse" ? entry : exit;
  const dialer = mode === "reverse" ? exit : entry;
  const parsed = useMemo(() => parsePorts(ports, protocol, target.trim() || "127.0.0.1"), [ports, protocol, target]);
  const greLink = netId
    ? links.find((l) => l.network === netId && [l.a, l.b].sort().join() === [entry, exit].sort().join())
    : undefined;
  const greAddr = greLink ? (greLink.a === exit ? greLink.addr_a : greLink.addr_b) : "";
  const udp = transport === "quic" || transport === "kcp";
  const isWs = transport === "ws" || transport === "wss";

  // The accepting server's address, as the other one reaches it: the panel's own is the
  // one the browser used.
  useEffect(() => {
    if (!editing && !dialHost && acceptor && servers.find((s) => s.id === acceptor)?.local) setDialHost(location.hostname);
  }, [acceptor]); // eslint-disable-line react-hooks/exhaustive-deps

  const request = (): PairRequest => ({
    name,
    entry,
    exit,
    mode,
    transport,
    profile,
    listen: `${listenHost}:${listenPort}`,
    dial: `${dialHost.includes(":") && !dialHost.startsWith("[") ? `[${dialHost}]` : dialHost}:${listenPort}`,
    network: mode === "direct" && netId ? netId : undefined,
    pool,
    ws_path: isWs ? wsPath : undefined,
    forwards: parsed.forwards,
  });

  const problem = (s: number): string => {
    if (s === 0) {
      if (!NAME.test(name)) return t("wz.nameHelp");
      if (!entry || !exit || entry === exit) return t("wz.samePick");
    }
    if (s === 2) {
      if (!/^\d{1,5}$/.test(listenPort) || +listenPort < 1 || +listenPort > 65535) return t("wz.port", { server: serverName(acceptor) });
      if (!dialHost.trim() && !netId) return t("wz.dial", { server: serverName(acceptor), other: serverName(dialer) });
      if (isWs && !wsPath.startsWith("/")) return t("wz.wsPath");
    }
    if (s === 3 && parsed.bad !== undefined) return parsed.bad ? t("wz.badPorts", { bit: parsed.bad }) : t("wz.ports");
    return "";
  };

  const go = async (to: number) => {
    setError("");
    if (to > step) {
      const p = problem(step);
      if (p) return setError(p);
    }
    setBack(to < step);
    setStep(to);
    if (to === 4 && !opId) {
      setChecking(true);
      setCheck(null);
      try {
        setCheck(await api.tunnelCheck(request()));
      } catch (e) {
        setError(e instanceof ApiError ? e.code : String(e));
      } finally {
        setChecking(false);
      }
    }
  };

  const conflict = (c: CheckReply | undefined, id: string) => {
    if (!c || c.ok) return "";
    const first = c.conflicts[0];
    return first ? t("wz.taken", { server: serverName(id), who: first.process ?? `port ${first.port}` }) : (c.error ?? "");
  };
  const conflicts = [conflict(check?.entry, entry), conflict(check?.exit, exit)].filter(Boolean);

  const build = async () => {
    setError("");
    try {
      const r = editing ? await api.editTunnel(request()) : await api.createTunnel(request());
      setOpId(r.op);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "busy" ? t("wz.busy") : e instanceof ApiError ? e.code : String(e));
    }
  };

  const running = !!opId && (!op || op.state === "running");
  const labels = [t("wz.s1"), t("wz.s2"), t("wz.s3"), t("wz.s4"), t("wz.s5")];
  const dir = lang === "fa" ? -1 : 1;

  return createPortal(
    <div ref={box} className={`wizard ${on ? "is-on" : ""}`} role="dialog" aria-modal="true" aria-label={editing ? t("wz.titleEdit", { name: edit!.name }) : t("wz.titleNew")}>
      <div className="wz-top">
        <h2>{editing ? t("wz.titleEdit", { name: edit!.name }) : t("wz.titleNew")}</h2>
        <button className="x-btn" type="button" onClick={onClose} disabled={running} aria-label={t("close")}>
          <Icon name="x" />
        </button>
      </div>
      <nav className="wz-steps" aria-label={t("tun.new")}>
        <div className="wz-channel" />
        <div className="wz-flow">
          <i style={{ width: `${(step / 4) * 100}%` }} />
        </div>
        <ol>
          {labels.map((label, i) => (
            <li key={label} className={i < step ? "done" : i === step ? "now" : ""} aria-current={i === step ? "step" : undefined} onClick={() => i < step && !running && !opId && void go(i)}>
              <span className="mound" />
              <span className="lbl">{label}</span>
            </li>
          ))}
        </ol>
      </nav>

      <div className="wz-body">
        <div className="wz-inner">
          <div className={`wz-step ${back ? "back" : ""}`} key={step} style={{ ["--dir" as string]: dir }}>
            {loading && <p className="muted">…</p>}

            {!loading && step === 0 && (
              <>
                <h3 className="wz-q">{t("wz.q1")}</h3>
                <p className="wz-lead">{t("wz.lead1")}</p>
                <div className="field" style={{ marginBottom: "var(--sp-5)", maxWidth: 360 }}>
                  <label htmlFor="wz-name">{t("wz.name")}</label>
                  <input className="text mono" id="wz-name" dir="ltr" value={name} disabled={editing} placeholder="tehran-frankfurt" onChange={(e) => setName(e.target.value.toLowerCase())} />
                  <span className="help">{t("wz.nameHelp")}</span>
                </div>
                <div className="pair">
                  <Picker label={t("wz.entry")} servers={servers} value={entry} other={exit} locked={editing} onPick={setEntry} />
                  <div className="pair-mid">
                    <svg viewBox="0 0 90 36" aria-hidden="true">
                      <path d="M4 18 H80" stroke="var(--water)" strokeWidth="2" strokeDasharray="4 6" fill="none" />
                      <path d="M74 10 L86 18 L74 26" stroke="var(--water)" strokeWidth="2" fill="none" />
                    </svg>
                  </div>
                  <Picker label={t("wz.exit")} servers={servers} value={exit} other={entry} locked={editing} onPick={setExit} />
                </div>
                {online.length < 2 && <p className="err small">{t("tun.needTwo")}</p>}
              </>
            )}

            {!loading && step === 1 && (
              <>
                <h3 className="wz-q">{t("wz.q2")}</h3>
                <p className="wz-lead">{t("wz.lead2")}</p>
                <div className="tiles" role="radiogroup" aria-label={t("wz.q2")}>
                  {(["reverse", "direct"] as const).map((m) => (
                    <button key={m} type="button" role="radio" aria-checked={mode === m} className="tile" onClick={() => setMode(m)}>
                      <span className="t1">{t(`wz.mode.${m}`)}</span>
                      <span className="t2">{t(`wz.mode.${m}.d`)}</span>
                      <span className="tick">
                        <Icon name="check" size={14} />
                      </span>
                    </button>
                  ))}
                </div>
                <h3 className="wz-q" style={{ marginTop: "var(--sp-6)", fontSize: "var(--fs-lg)" }}>
                  {t("wz.transport")}
                </h3>
                <div className="tiles" role="radiogroup" aria-label={t("wz.transport")}>
                  {TRANSPORTS.map((x) => (
                    <button key={x} type="button" role="radio" aria-checked={transport === x} className="tile" onClick={() => setTransport(x)}>
                      <span className="t1 mono">{x}</span>
                      <span className="t2">{t(`wz.tr.${x}`)}</span>
                      <span className="tick">
                        <Icon name="check" size={14} />
                      </span>
                    </button>
                  ))}
                </div>
                <div className="field" style={{ marginTop: "var(--sp-6)" }}>
                  <span className="label">{t("wz.profile")}</span>
                  <Seg value={profile} options={PROFILES.map((p) => [p, p] as [string, string])} onChange={setProfile} />
                </div>
              </>
            )}

            {!loading && step === 2 && (
              <>
                <h3 className="wz-q">{t("wz.q3")}</h3>
                <p className="wz-lead">{t("wz.lead3", { acceptor: serverName(acceptor), dialer: serverName(dialer) })}</p>
                {mode === "direct" && !editing && (
                  <div className="field" style={{ marginBottom: "var(--sp-5)" }}>
                    <label className="check">
                      <input type="checkbox" checked={!!netId} disabled={networks.length === 0} onChange={(e) => setNetId(e.target.checked ? networks[0]?.id ?? "" : "")} /> {t("wz.gre")}
                    </label>
                    {networks.length === 0 && <span className="help">{t("wz.greNone")}</span>}
                    {netId && (
                      <>
                        <select className="select" aria-label={t("wz.greNet")} value={netId} onChange={(e) => setNetId(e.target.value)} style={{ maxWidth: 360 }}>
                          {networks.map((n) => (
                            <option key={n.id} value={n.id}>
                              {n.name} ({n.cidr})
                            </option>
                          ))}
                        </select>
                        <span className="help">{t("wz.greText")}</span>
                        <span className="help mono" dir="ltr">
                          {greLink ? t("wz.greAddrs", { addr: `${greAddr}:${listenPort}`, server: serverName(exit) }) : t("wz.greNew")}
                        </span>
                      </>
                    )}
                  </div>
                )}
                <div className="grid-2" style={{ display: "grid", gap: "var(--sp-5)", gridTemplateColumns: "1fr 1fr" }}>
                  <div className="field">
                    <label htmlFor="wz-port">{t("wz.port", { server: serverName(acceptor) })}</label>
                    <input className="text mono" id="wz-port" dir="ltr" inputMode="numeric" value={listenPort} onChange={(e) => setListenPort(e.target.value.trim())} />
                  </div>
                  <div className="field" hidden={!!netId}>
                    <label htmlFor="wz-dial">{t("wz.dial", { server: serverName(acceptor), other: serverName(dialer) })}</label>
                    <input className="text mono" id="wz-dial" dir="ltr" value={dialHost} placeholder="203.0.113.5" onChange={(e) => setDialHost(e.target.value.trim())} />
                    <span className="help">{t("wz.dialHelp")}</span>
                  </div>
                  {isWs && (
                    <div className="field">
                      <label htmlFor="wz-ws">{t("wz.wsPath")}</label>
                      <input className="text mono" id="wz-ws" dir="ltr" value={wsPath} onChange={(e) => setWsPath(e.target.value.trim())} />
                    </div>
                  )}
                </div>
                {udp && <p className="muted small">UDP</p>}
              </>
            )}

            {!loading && step === 3 && (
              <>
                <h3 className="wz-q">{t("wz.q4")}</h3>
                <p className="wz-lead">{t("wz.lead4")}</p>
                <div className="field" style={{ maxWidth: 520 }}>
                  <label htmlFor="wz-ports">{t("wz.ports")}</label>
                  <input className="text mono" id="wz-ports" dir="ltr" value={ports} placeholder="443, 8080-8090" onChange={(e) => setPorts(e.target.value)} />
                  <span className="help">{t("wz.portsHelp")}</span>
                </div>
                <div className="port-chips" style={{ margin: "var(--sp-4) 0" }} aria-live="polite">
                  {parsed.forwards.slice(0, 40).map((f) => (
                    <span className="pchip" key={f.listen + f.target}>
                      {f.listen.split(":").pop()}
                      <small>→ {f.target.split(":").pop()}</small>
                    </span>
                  ))}
                  {parsed.forwards.length > 40 && <span className="pchip">+{num(parsed.forwards.length - 40)}</span>}
                  {parsed.bad && <span className="pchip bad">{parsed.bad}</span>}
                </div>
                <div className="grid-2" style={{ display: "grid", gap: "var(--sp-5)", gridTemplateColumns: "1fr 1fr", maxWidth: 520 }}>
                  <div className="field">
                    <span className="label">{t("wz.protocol")}</span>
                    <Seg value={protocol} options={[["tcp", "TCP"], ["udp", "UDP"], ["tcp+udp", "TCP+UDP"]]} onChange={setProtocol} />
                  </div>
                  <div className="field">
                    <label htmlFor="wz-target">{t("wz.targetHost")}</label>
                    <input className="text mono" id="wz-target" dir="ltr" value={target} onChange={(e) => setTarget(e.target.value.trim())} />
                  </div>
                </div>
              </>
            )}

            {!loading && step === 4 && (
              <>
                <h3 className="wz-q">{opId ? name : t("wz.q5")}</h3>
                {!opId && <p className="wz-lead">{t("wz.lead5")}</p>}
                {!opId && (
                  <div className="review">
                    {(["entry", "exit"] as const).map((side) => {
                      const id = side === "entry" ? entry : exit;
                      const accepts = id === acceptor;
                      return (
                        <div className="review-card" key={side}>
                          <h4>
                            {t(`wz.review.${side}`)} · {serverName(id)}
                          </h4>
                          <dl className="kv">
                            <dt>{t("wz.transport")}</dt>
                            <dd className="mono">
                              {transport} · {mode} · {profile}
                            </dd>
                            <dt>{accepts ? "⇢" : "⇠"}</dt>
                            <dd className="mono" dir="ltr">
                              {netId ? (greAddr ? `${greAddr}:${listenPort}` : "GRE") : accepts ? `${listenHost}:${listenPort}` : `${dialHost}:${listenPort}`}
                            </dd>
                            {side === "entry" && (
                              <>
                                <dt>{t("tun.forwards")}</dt>
                                <dd className="mono" dir="ltr">
                                  {parsed.forwards.length ? ports : "—"}
                                </dd>
                              </>
                            )}
                          </dl>
                        </div>
                      );
                    })}
                  </div>
                )}
                {checking && <p className="muted small">…</p>}
                {!opId && conflicts.map((c) => (
                  <p className="err small" key={c}>
                    {c}
                  </p>
                ))}
                <Checklist op={op} />
                {op?.state === "done" && <p className="ok small">{t("tun.done")}</p>}
                {op?.state === "failed" && (
                  <>
                    <p className="err small">{t("tun.failed", { why: opError(t, op.error, new Map(servers.map((x) => [x.id, x.name]))) })}</p>
                    <p className="muted small">{op.undone ? t("tun.undone") : t("tun.notUndone")}</p>
                  </>
                )}
              </>
            )}
            {error && <p className="err small" role="alert">{error}</p>}
          </div>
        </div>
      </div>

      <div className="wz-foot">
        {step > 0 && !opId && (
          <button className="btn btn-ghost" type="button" onClick={() => void go(step - 1)}>
            {t("wz.back")}
          </button>
        )}
        <span className="grow" />
        {op?.state === "done" && (
          <button className="btn btn-primary" type="button" onClick={onClose}>
            {t("wz.close")}
          </button>
        )}
        {op?.state === "failed" && (
          <>
            <button className="btn btn-ghost" type="button" onClick={onClose}>
              {t("wz.close")}
            </button>
            <button className="btn btn-primary" type="button" onClick={() => setOpId(null)}>
              {t("wz.retry")}
            </button>
          </>
        )}
        {!opId && step < 4 && (
          <button className="btn btn-primary" type="button" disabled={loading} onClick={() => void go(step + 1)}>
            {t("wz.next")}
          </button>
        )}
        {!opId && step === 4 && (
          <button className="btn btn-primary" type="button" disabled={checking || conflicts.length > 0} onClick={() => void build()}>
            <span className="shine" />
            {editing ? t("wz.save") : t("wz.make")}
          </button>
        )}
      </div>
    </div>,
    document.body,
  );
}

function Picker({ label, servers, value, other, locked, onPick }: { label: string; servers: ServerInfo[]; value: string; other: string; locked: boolean; onPick: (id: string) => void }) {
  const { t } = useApp();
  return (
    <div className="field">
      <span className="label">{label}</span>
      <div className="pick-list" role="radiogroup" aria-label={label}>
        {servers.map((s) => {
          const disabled = locked || !s.online || s.id === other;
          return (
            <button key={s.id} type="button" role="radio" aria-checked={value === s.id} aria-disabled={disabled} className="pick" onClick={() => !disabled && onPick(s.id)}>
              <span className="radio" />
              <span>
                <span className="nm">{s.name}</span>
                <br />
                <span className="mt">{s.online ? `${s.arch} · Kariz ${s.version}` : t("wz.offline")}</span>
              </span>
              <span />
            </button>
          );
        })}
      </div>
    </div>
  );
}
