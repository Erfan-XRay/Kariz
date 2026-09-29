import { useEffect, useMemo, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { ServerInfo, SessionRow } from "./api";
import { bytesPerSec, pairTunnels, rateParts } from "./derive";
import { paletteNow } from "./draw";
import { startMap } from "./scene-map";
import type { MapData, MapHit } from "./scene-map";
import { useApp } from "./store";
import { BackupDialog, RestoreDialog } from "./Extras";
import { CodeBlock, Dialog, Icon, Odo, Seg, useAgo } from "./ui";

// ---------------------------------------------------------------- the map

const stateKey = { up: "st.up", down: "st.down", off: "st.off" } as const;

export function MapPage({ servers }: { servers: ServerInfo[] }) {
  const { t, num, low } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [tip, setTip] = useState<MapHit | null>(null);
  const tunnels = useMemo(() => pairTunnels(servers), [servers]);
  // The scene reads the latest data and settings without restarting.
  const data = useRef<MapData>({ servers: [], tunnels: [] });
  const lowRef = useRef(low);
  data.current = {
    servers: servers.map((s) => ({ id: s.id, name: s.name, sub: s.local ? t("role.local") : `${s.arch} · ${s.version}`, st: s.online ? ("up" as const) : ("down" as const) })),
    tunnels: tunnels
      .filter((x) => x.paired)
      .map((x) => {
        const r = rateParts(x.rate);
        return {
          id: x.name,
          a: x.entry!.server.id,
          b: x.exit!.server.id,
          st: x.state,
          rate: x.rate,
          label: x.state === "up" ? `${x.name}  ${num(r.value, r.decimals)} ${r.unit}` : `${x.name}  ${t(stateKey[x.state])}`,
        };
      }),
  };
  lowRef.current = low;

  useEffect(() => {
    if (!canvas.current) return;
    return startMap(canvas.current, {
      data: () => data.current,
      palette: paletteNow,
      low: () => lowRef.current,
      rtl: () => document.documentElement.dir === "rtl",
      onHover: setTip,
      onOpen: () => {},
    });
  }, []);

  const up = tunnels.filter((x) => x.state === "up");
  const total = up.reduce((n, x) => n + x.rate, 0);
  const rate = rateParts(total);
  const conns = up.reduce((n, x) => n + x.connections, 0);
  const online = servers.filter((s) => s.online).length;
  const hoveredServer = tip?.kind === "server" ? servers.find((s) => s.id === tip.id) : undefined;
  const hoveredTunnel = tip?.kind === "tunnel" ? tunnels.find((x) => x.name === tip.id) : undefined;
  return (
    <div className="page is-on">
      <div className="map-band">
        <canvas id="map" ref={canvas} role="img" aria-describedby="map-alt" />
        <div className="map-legend">
          <span>
            <i className="dot up" />
            {t("st.up")}
          </span>
          <span>
            <i className="dot down" />
            {t("st.down")}
          </span>
          <span>
            <i className="dot off" />
            {t("st.off")}
          </span>
        </div>
        {tip && (hoveredServer || hoveredTunnel) && (
          <div className="map-tip is-on" style={{ left: Math.min(tip.x + 16, 600), top: tip.y + 16 }}>
            {hoveredServer && (
              <>
                <b>{hoveredServer.name}</b>
                <div className="row">
                  <span>{t("srv.cpu")}</span>
                  <span className="num">{hoveredServer.health?.cpu_pct != null ? `${num(Math.round(hoveredServer.health.cpu_pct))}%` : "—"}</span>
                </div>
                <div className="row">
                  <span>{t("srv.ram")}</span>
                  <span className="num">{hoveredServer.health?.mem_total ? `${num(Math.round(((hoveredServer.health.mem_used ?? 0) * 100) / hoveredServer.health.mem_total))}%` : "—"}</span>
                </div>
                <div className="row">
                  <span>{t("tip.tunnels")}</span>
                  <span className="num">{num(hoveredServer.tunnels.length)}</span>
                </div>
              </>
            )}
            {hoveredTunnel && (
              <>
                <b>{hoveredTunnel.name}</b>
                <div className="row">
                  <span className="mono">
                    {hoveredTunnel.entry?.server.name} → {hoveredTunnel.exit?.server.name}
                  </span>
                </div>
                <div className="row">
                  <span>{t("t.transport")}</span>
                  <span className="mono">
                    {hoveredTunnel.transport} · {hoveredTunnel.profile}
                  </span>
                </div>
                {hoveredTunnel.rtt != null && (
                  <div className="row">
                    <span>rtt</span>
                    <span className="num">{num(hoveredTunnel.rtt, 1)} ms</span>
                  </div>
                )}
                {hoveredTunnel.error && <div className="row" style={{ color: "var(--danger)" }}>{hoveredTunnel.error}</div>}
              </>
            )}
          </div>
        )}
        <p className="sr-only" id="map-alt">
          {t("alt.prefix")} {servers.map((s) => s.name).join(", ")}; {tunnels.map((x) => `${x.name}: ${t(stateKey[x.state])}`).join(", ")}
        </p>
      </div>

      <div className="stratum s1">
        <div className="metrics">
          <div className="metric">
            <div className="label">{t("m.through")}</div>
            <div className="value num">
              <Odo value={num(rate.value, rate.decimals)} />
              <span className="unit">{rate.unit}</span>
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.conns")}</div>
            <div className="value num">
              <Odo value={num(conns)} />
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.tunnels")}</div>
            <div className="value num">
              <Odo value={num(up.length)} />
              <span className="unit">
                {t("m.of")} {num(tunnels.length)}
              </span>
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.servers")}</div>
            <div className="value num">
              <Odo value={num(online)} />
              <span className="unit">
                {t("m.of")} {num(servers.length)}
              </span>
            </div>
          </div>
        </div>
      </div>

      <div className="stratum s2">
        <div className="stratum-head">
          <h2>{t("t.title")}</h2>
          <span className="eyebrow">{t("t.hint")}</span>
        </div>
        {tunnels.length === 0 ? (
          <p className="muted small" style={{ margin: 0, maxWidth: 640 }}>
            {t("t.empty")}
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
                  <tr key={x.name}>
                    <td>
                      <span className="t-name">{x.name}</span>
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
    </div>
  );
}

// ---------------------------------------------------------------- servers

function Well({ level = 0.6, online = true }: { level?: number; online?: boolean }) {
  const y = 40 - 14 * level;
  return (
    <svg className="srv-well" viewBox="0 0 44 44" aria-hidden="true">
      <path d="M4 16h36" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" />
      <path d="M12 16a10 7 0 0 1 20 0Z" fill="var(--accent)" />
      <rect x="18" y="16" width="8" height="25" rx="2" fill="var(--stratum-3)" />
      <rect x="18" y={y} width="8" height={41 - y} rx="2" fill={online ? "var(--water)" : "var(--text-3)"} />
    </svg>
  );
}

function Meter({ label, pct }: { label: string; pct: number | null }) {
  const { num } = useApp();
  const v = pct == null ? 0 : Math.max(0, Math.min(100, pct));
  return (
    <div className={`meter ${v > 85 ? "hot" : ""}`}>
      <div className="top">
        <span>{label}</span>
        <b>{pct == null ? "—" : `${num(Math.round(v))}%`}</b>
      </div>
      <div className="bar">
        <i style={{ width: `${v}%` }} />
      </div>
    </div>
  );
}

function AddServer({ servers, agentsOn, onClose }: { servers: ServerInfo[]; agentsOn: boolean; onClose: () => void }) {
  const { t, digitsOf, toast } = useApp();
  const [name, setName] = useState("");
  const [host, setHost] = useState(location.hostname);
  const [code, setCode] = useState("");
  const [left, setLeft] = useState(0);
  const [error, setError] = useState("");
  const known = useRef<Set<string>>(new Set());
  const joined = servers.find((s) => code && !known.current.has(s.id) && s.online);

  useEffect(() => {
    if (left <= 0 || joined) return;
    const id = setInterval(() => setLeft((v) => Math.max(0, v - 1)), 1000);
    return () => clearInterval(id);
  }, [left > 0, !!joined]); // eslint-disable-line react-hooks/exhaustive-deps

  const make = async () => {
    setError("");
    try {
      known.current = new Set(servers.map((s) => s.id));
      const made = await api.joinCode(name.trim() || undefined, host.trim());
      setCode(made.code);
      setLeft(made.valid_for);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "agents_off" ? t("add.off") : t("add.bad"));
    }
  };

  const mmss = `${String(Math.floor(left / 60)).padStart(2, "0")}:${String(left % 60).padStart(2, "0")}`;
  return (
    <Dialog
      title={t("add.title")}
      onClose={onClose}
      footer={
        joined ? (
          <button
            className="btn btn-primary btn-sm"
            type="button"
            onClick={() => {
              toast(t("add.connected", { name: joined.name }));
              onClose();
            }}
          >
            {t("add.done")}
          </button>
        ) : (
          <>
            <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
              {t("cancel")}
            </button>
            {!code && (
              <button className="btn btn-primary btn-sm" type="button" disabled={!host.trim() || !agentsOn} onClick={() => void make()}>
                {t("add.make")}
              </button>
            )}
          </>
        )
      }
    >
      {!agentsOn && <p className="err small">{t("add.off")}</p>}
      {!code && (
        <>
          <div className="field">
            <label htmlFor="add-name">{t("add.name")}</label>
            <input className="text mono" id="add-name" dir="ltr" placeholder="istanbul-1" value={name} onChange={(e) => setName(e.target.value)} />
            <span className="help">{t("add.nameHelp")}</span>
          </div>
          <div className="field">
            <label htmlFor="add-host">{t("add.host")}</label>
            <input className="text mono" id="add-host" dir="ltr" value={host} onChange={(e) => setHost(e.target.value)} />
            <span className="err">{error}</span>
          </div>
        </>
      )}
      {code && (
        <>
          <div className="field">
            <span className="label">{t("add.run")}</span>
            <CodeBlock text={`kariz-panel agent --join ${code}`} />
            <span className="countdown">{joined ? "" : t("add.valid", { m: digitsOf(mmss) })}</span>
          </div>
          <div className={`waiting ${joined ? "done" : ""}`}>
            {!joined && (
              <svg className="qloader" viewBox="0 0 180 90" aria-hidden="true">
                <path className="bed" d="M30 58 L158 76" />
                <path className="flow" pathLength={1} d="M30 58 L158 76" />
                <path className="shaft" d="M38 26 V58 M84 26 V64 M130 26 V70" />
                <path className="ground" d="M8 26 H172" />
                <path className="mound" d="M30 26 a8 6 0 0 1 16 0Z M76 26 a8 6 0 0 1 16 0Z M122 26 a8 6 0 0 1 16 0Z" />
                <circle className="drop" cx="38" cy="26" r="4" />
                <path className="out" d="M164 68 L178 77 L164 86 Z" />
              </svg>
            )}
            <span>{joined ? t("add.connected", { name: joined.name }) : t("add.waiting")}</span>
          </div>
        </>
      )}
    </Dialog>
  );
}

export function ServersPage({ servers, agentsOn, onChanged }: { servers: ServerInfo[]; agentsOn: boolean; onChanged: () => void }) {
  const { t, num, toast, digitsOf } = useApp();
  const ago = useAgo();
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<ServerInfo | null>(null);
  return (
    <div className="page is-on">
      <div className="page-band">
        <div className="summary">
          <div>
            <b>{num(servers.length)}</b>
            <span>{t("m.serversNote")}</span>
          </div>
          <div>
            <b>{num(servers.filter((s) => s.online).length)}</b>
            <span>{t("srv.online")}</span>
          </div>
        </div>
        <div className="grow" />
        <button className="btn btn-primary btn-sm" type="button" onClick={() => setAdding(true)}>
          <span className="shine" />
          <Icon name="plus" size={18} />
          {t("srv.add")}
        </button>
      </div>
      {servers.length === 0 && (
        <div className="stratum s1">
          <p className="muted">{t("srv.none")}</p>
        </div>
      )}
      <ul className="srv-list">
        {servers.map((s) => {
          const h = s.health;
          const mem = h?.mem_total ? ((h.mem_used ?? 0) * 100) / h.mem_total : null;
          const rx = h?.rx_bps != null ? bytesPerSec(h.rx_bps) : null;
          const tx = h?.tx_bps != null ? bytesPerSec(h.tx_bps) : null;
          return (
            <li className="srv" key={s.id}>
              <Well level={h?.cpu_pct != null ? Math.min(1, h.cpu_pct / 100 + 0.3) : 0.5} online={s.online} />
              <div>
                <div className="srv-name">{s.name}</div>
                <div className="srv-sub">
                  {t("srv.version", { v: s.version })} · {s.arch}
                  {h?.uptime_secs != null && ` · ${t("srv.up")} ${digitsOf(Math.floor(h.uptime_secs / 86400) > 0 ? `${Math.floor(h.uptime_secs / 86400)} d` : `${Math.floor(h.uptime_secs / 3600)} h`)}`}
                </div>
              </div>
              <div className="c-link">
                <div className="srv-link">
                  <i className={`dot ${s.online ? "up" : "down"}`} />
                  <span>{s.local ? t("srv.panel") : s.online ? t("srv.online") : `${t("srv.offline")}${s.seen_secs != null ? ` · ${ago(s.seen_secs)}` : ""}`}</span>
                </div>
              </div>
              <div className="m-cpu">
                <Meter label={t("srv.cpu")} pct={h?.cpu_pct ?? null} />
              </div>
              <div className="m-ram">
                <Meter label={t("srv.ram")} pct={mem} />
              </div>
              <div className="m-net">
                <div className="meter">
                  <div className="top">
                    <span>{t("srv.net")}</span>
                  </div>
                  <span className="num small" dir="ltr">
                    {rx && tx ? `↓ ${num(rx.value, rx.decimals)} ↑ ${num(tx.value, tx.decimals)} ${rx.unit}` : "—"}
                  </span>
                </div>
              </div>
              <div style={{ display: "flex", gap: "var(--sp-2)", alignItems: "center" }}>
                <span className={`badge ${s.tunnels.length ? "" : "muted"}`}>{t(s.tunnels.length === 1 ? "srv.tn1" : "srv.tn", { n: num(s.tunnels.length) })}</span>
                {!s.local && (
                  <button className="btn btn-ghost btn-sm" type="button" onClick={() => setRemoving(s)}>
                    {t("srv.remove")}
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {adding && <AddServer servers={servers} agentsOn={agentsOn} onClose={() => setAdding(false)} />}
      {removing && (
        <Dialog
          title={t("srv.removeTitle", { name: removing.name })}
          onClose={() => setRemoving(null)}
          footer={
            <>
              <button className="btn btn-ghost btn-sm" type="button" onClick={() => setRemoving(null)}>
                {t("cancel")}
              </button>
              <button
                className="btn btn-danger solid btn-sm"
                type="button"
                onClick={async () => {
                  const gone = removing;
                  setRemoving(null);
                  await api.removeServer(gone.id).catch(() => {});
                  toast(t("srv.removed", { name: gone.name }));
                  onChanged();
                }}
              >
                {t("srv.remove")}
              </button>
            </>
          }
        >
          <p className="small" style={{ margin: 0 }}>
            {t("srv.removeText")}
          </p>
        </Dialog>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- pages that come later

export function Later({ phase }: { phase: string }) {
  const { t } = useApp();
  return (
    <div className="page is-on">
      <div className="stratum s1">
        <div className="placeholder">
          <svg className="qloader" viewBox="0 0 180 90" aria-hidden="true">
            <path className="bed" d="M30 58 L158 76" />
            <path className="flow" pathLength={1} d="M30 58 L158 76" />
            <path className="shaft" d="M38 26 V58 M84 26 V64 M130 26 V70" />
            <path className="ground" d="M8 26 H172" />
            <path className="mound" d="M30 26 a8 6 0 0 1 16 0Z M76 26 a8 6 0 0 1 16 0Z M122 26 a8 6 0 0 1 16 0Z" />
            <circle className="drop" cx="38" cy="26" r="4" />
            <path className="out" d="M164 68 L178 77 L164 86 Z" />
          </svg>
          <p>{t("soon", { phase })}</p>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- settings

function strength(pw: string): number {
  let s = 0;
  if (pw.length >= 10) s++;
  if (pw.length >= 14) s++;
  if (/[A-Z]/.test(pw) && /[a-z]/.test(pw)) s++;
  if (/\d/.test(pw) && /[^\w]/.test(pw)) s++;
  return s;
}

function PasswordDialog({ hasPassword, onClose, onSaved }: { hasPassword: boolean; onClose: () => void; onSaved: () => void }) {
  const { t } = useApp();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const s = strength(next);
  const mismatch = again !== "" && again !== next;
  const ok = next.length >= 10 && next === again && (!hasPassword || current !== "");

  const save = async () => {
    setBusy(true);
    setError("");
    try {
      await api.changePassword(hasPassword ? current : undefined, next);
      onSaved();
    } catch (e) {
      setBusy(false);
      if (e instanceof ApiError && e.code === "wrong") setError(t("set.pwWrong"));
      else if (e instanceof ApiError && e.code === "too_short") setError(t("set.pwShort"));
      else if (e instanceof ApiError && e.code === "locked") setError(t("login.locked", { m: Math.ceil(Number(e.body.retry_after ?? 900) / 60) }));
      else setError(t("login.error"));
    }
  };

  return (
    <Dialog
      title={hasPassword ? t("set.pwChange") : t("set.pwSetBtn")}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={!ok || busy} onClick={save}>
            {hasPassword ? t("set.pwChange") : t("set.pwSetBtn")}
          </button>
        </>
      }
    >
      {hasPassword && (
        <div className="field">
          <label htmlFor="pw0">{t("set.pwCur")}</label>
          <input className="text" id="pw0" type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} />
        </div>
      )}
      <div className="field">
        <label htmlFor="pw1">{t("set.pwNew")}</label>
        <input className="text" id="pw1" type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} />
        <div className={`meter ${s < 2 && next ? "hot" : ""}`}>
          <div className="top">
            <span />
            <b>{next ? t(s < 2 ? "set.pwWeak" : s < 3 ? "set.pwOk" : "set.pwStrong") : ""}</b>
          </div>
          <div className="bar">
            <i style={{ width: `${(s / 4) * 100}%` }} />
          </div>
        </div>
      </div>
      <div className="field">
        <label htmlFor="pw2">{t("set.pwAgain")}</label>
        <input className="text" id="pw2" type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
        <span className="err">{mismatch ? t("set.pwMismatch") : error}</span>
      </div>
    </Dialog>
  );
}

function LoginLink() {
  const { t, digitsOf } = useApp();
  const [url, setUrl] = useState("");
  const [left, setLeft] = useState(0);

  useEffect(() => {
    if (left <= 0) return;
    const id = setInterval(() => setLeft((v) => Math.max(0, v - 1)), 1000);
    return () => clearInterval(id);
  }, [left > 0]); // eslint-disable-line react-hooks/exhaustive-deps

  const make = async () => {
    const { token, valid_for } = await api.newLink();
    setUrl(`${location.origin}${location.pathname}#t=${token}`);
    setLeft(valid_for);
  };
  const mmss = `${String(Math.floor(left / 60)).padStart(2, "0")}:${String(left % 60).padStart(2, "0")}`;
  return (
    <>
      <button className="btn btn-ghost btn-sm" type="button" onClick={() => void make()}>
        <Icon name="key" size={18} />
        {t("set.linkMake")}
      </button>
      {url && (
        <div style={{ width: "100%" }}>
          <CodeBlock text={url} />
          <span className="countdown">{left > 0 ? t("set.linkValid", { m: digitsOf(mmss) }) : t("set.linkExpired")}</span>
        </div>
      )}
    </>
  );
}

/** "Edge · Windows" out of a User-Agent string. */
export function deviceName(ua: string): string {
  const browser =
    /Edg\//.test(ua) ? "Edge" : /OPR\/|Opera/.test(ua) ? "Opera" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : /curl/i.test(ua) ? "curl" : "";
  const os = /Windows/.test(ua)
    ? "Windows"
    : /Android/.test(ua)
      ? "Android"
      : /iPhone|iPad/.test(ua)
        ? "iOS"
        : /Mac OS X|Macintosh/.test(ua)
          ? "macOS"
          : /Linux/.test(ua)
            ? "Linux"
            : "";
  return [browser, os].filter(Boolean).join(" · ") || ua.slice(0, 40) || "?";
}

function Sessions() {
  const { t, toast } = useApp();
  const ago = useAgo();
  const [rows, setRows] = useState<SessionRow[]>([]);
  const load = () => api.sessions().then((r) => setRows(r.sessions)).catch(() => {});
  useEffect(() => {
    void load();
  }, []);
  const now = Date.now() / 1000;
  return (
    <div style={{ width: "100%" }}>
      {rows.map((s) => (
        <div className="session" key={s.id}>
          <span className="who">
            {deviceName(s.agent)} {s.current && <span className="badge water">{t("set.thisDevice")}</span>}
          </span>
          {s.current ? (
            <span />
          ) : (
            <button
              className="btn btn-ghost btn-sm"
              type="button"
              onClick={async () => {
                await api.revoke(s.id);
                toast(t("set.revoked"));
                void load();
              }}
            >
              {t("set.revoke")}
            </button>
          )}
          <span className="meta">
            <span className="mono" dir="ltr">
              {s.ip}
            </span>{" "}
            · {s.current ? t("now") : ago(now - s.last_seen)}
          </span>
        </div>
      ))}
    </div>
  );
}

export function SettingsPage({ hasPassword, onPasswordSet }: { hasPassword: boolean; onPasswordSet: () => void }) {
  const { t, lang, setLang, theme, setTheme, digits, setDigits, low, setLow, toast } = useApp();
  const [dialog, setDialog] = useState(false);
  const [backup, setBackup] = useState<"save" | "restore" | null>(null);
  const row = (title: string, text: string, control: React.ReactNode) => (
    <div className="set-row">
      <div>
        <h3>{title}</h3>
        {text && <p>{text}</p>}
      </div>
      <div className="ctl">{control}</div>
    </div>
  );
  const security = useMemo(() => t("set.sec"), [t]);
  return (
    <div className="page is-on">
      <section className="stratum s1">
        <div className="stratum-head">
          <h2>{security}</h2>
        </div>
        {row(
          t("set.pw"),
          t(hasPassword ? "set.pwSet" : "set.pwNone"),
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => setDialog(true)}>
            <Icon name="key" size={18} />
            {t(hasPassword ? "set.pwChange" : "set.pwSetBtn")}
          </button>,
        )}
        {row(t("set.link"), t("set.linkText"), <LoginLink />)}
        {row(t("set.sessions"), t("set.sessionsText"), <Sessions />)}
      </section>
      <section className="stratum s2">
        <div className="stratum-head">
          <h2>{t("set.backup")}</h2>
        </div>
        {row(
          t("set.backupSave"),
          t("set.backupSaveText"),
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => setBackup("save")}>
            {t("bak.download")}
          </button>,
        )}
        {row(
          t("set.backupRestore"),
          t("set.backupRestoreText"),
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => setBackup("restore")}>
            {t("rst.go")}
          </button>,
        )}
      </section>
      <section className="stratum s3">
        <div className="stratum-head">
          <h2>{t("set.look")}</h2>
        </div>
        {row(t("set.lang"), "", <Seg value={lang} options={[["fa", "فارسی"], ["en", "English"]]} onChange={setLang} />)}
        {row(t("set.theme"), "", <Seg value={theme} options={[["night", t("set.night")], ["dawn", t("set.dawn")]]} onChange={setTheme} />)}
        {lang === "fa" && row(t("set.digits"), "", <Seg value={digits} options={[["fa", t("set.digitsFa")], ["latin", t("set.digitsLatin")]]} onChange={setDigits} />)}
        {row(t("set.motion"), "", <Seg value={low ? "low" : "full"} options={[["full", t("set.motionFull")], ["low", t("set.motionLow")]]} onChange={(v) => setLow(v === "low", true)} />)}
      </section>
      {backup === "save" && <BackupDialog onClose={() => setBackup(null)} />}
      {backup === "restore" && <RestoreDialog onClose={() => setBackup(null)} onDone={() => {}} />}
      {dialog && (
        <PasswordDialog
          hasPassword={hasPassword}
          onClose={() => setDialog(false)}
          onSaved={() => {
            setDialog(false);
            onPasswordSet();
            toast(t("set.pwSaved"));
          }}
        />
      )}
    </div>
  );
}
