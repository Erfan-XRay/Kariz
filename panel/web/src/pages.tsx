import { useEffect, useMemo, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { ServerInfo, SessionRow } from "./api";
import { paletteNow } from "./draw";
import { startMap } from "./scene-map";
import type { MapData, MapHit } from "./scene-map";
import { useApp } from "./store";
import { CodeBlock, Dialog, Icon, Odo, Seg, useAgo } from "./ui";

// ---------------------------------------------------------------- the map

export function MapPage({ servers }: { servers: ServerInfo[] }) {
  const { t, num, low } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const [tip, setTip] = useState<MapHit | null>(null);
  // The scene reads the latest data and settings without restarting.
  const data = useRef<MapData>({ servers: [], tunnels: [] });
  const lowRef = useRef(low);
  data.current = {
    servers: servers.map((s) => ({ id: s.id, name: s.name, sub: s.local ? t("role.local") : "", st: "up" as const })),
    tunnels: [],
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

  const hovered = tip?.kind === "server" ? servers.find((s) => s.id === tip.id) : undefined;
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
        {hovered && tip && (
          <div className="map-tip is-on" style={{ left: Math.min(tip.x + 16, 600), top: tip.y + 16 }}>
            <b>{hovered.name}</b>
            <div className="row">
              <span>{t("tip.role")}</span>
              <span>{t("role.local")}</span>
            </div>
            <div className="row">
              <span>{t("srv.version", { v: hovered.version })}</span>
              <span className="mono">{hovered.arch}</span>
            </div>
          </div>
        )}
        <p className="sr-only" id="map-alt">
          {t("alt.prefix")} {servers.map((s) => s.name).join(", ")}
        </p>
      </div>

      <div className="stratum s1">
        <div className="metrics">
          <div className="metric">
            <div className="label">{t("m.through")}</div>
            <div className="value num">
              <Odo value={num(0, 1)} />
              <span className="unit">Mbps</span>
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.conns")}</div>
            <div className="value num">
              <Odo value={num(0)} />
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.tunnels")}</div>
            <div className="value num">
              <Odo value={num(0)} />
              <span className="unit">
                {t("m.of")} {num(0)}
              </span>
            </div>
          </div>
          <div className="metric">
            <div className="label">{t("m.servers")}</div>
            <div className="value num">
              <Odo value={num(servers.length)} />
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
        </div>
        <p className="muted small" style={{ margin: 0, maxWidth: 640 }}>
          {t("t.empty")}
        </p>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- servers

function Well({ level = 0.6 }: { level?: number }) {
  const y = 40 - 14 * level;
  return (
    <svg className="srv-well" viewBox="0 0 44 44" aria-hidden="true">
      <path d="M4 16h36" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" />
      <path d="M12 16a10 7 0 0 1 20 0Z" fill="var(--accent)" />
      <rect x="18" y="16" width="8" height="25" rx="2" fill="var(--stratum-3)" />
      <rect x="18" y={y} width="8" height={41 - y} rx="2" fill="var(--water)" />
    </svg>
  );
}

export function ServersPage({ servers }: { servers: ServerInfo[] }) {
  const { t } = useApp();
  return (
    <div className="page is-on">
      <div className="page-band">
        <div className="grow" />
        <button className="btn btn-primary btn-sm" type="button" disabled title={t("srv.addSoon")}>
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
        {servers.map((s) => (
          <li className="srv" key={s.id} style={{ gridTemplateColumns: "56px minmax(180px, 1.3fr) auto" }}>
            <Well />
            <div>
              <div className="srv-name">{s.name}</div>
              <div className="srv-sub">
                {t("srv.version", { v: s.version })} · {s.arch}
              </div>
            </div>
            <span className="badge water">{s.local ? t("srv.panel") : ""}</span>
          </li>
        ))}
      </ul>
      <div className="stratum s2">
        <p className="muted small" style={{ margin: 0 }}>
          {t("srv.addSoon")}
        </p>
      </div>
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
          <h2>{t("set.look")}</h2>
        </div>
        {row(t("set.lang"), "", <Seg value={lang} options={[["fa", "فارسی"], ["en", "English"]]} onChange={setLang} />)}
        {row(t("set.theme"), "", <Seg value={theme} options={[["night", t("set.night")], ["dawn", t("set.dawn")]]} onChange={setTheme} />)}
        {lang === "fa" && row(t("set.digits"), "", <Seg value={digits} options={[["fa", t("set.digitsFa")], ["latin", t("set.digitsLatin")]]} onChange={setDigits} />)}
        {row(t("set.motion"), "", <Seg value={low ? "low" : "full"} options={[["full", t("set.motionFull")], ["low", t("set.motionLow")]]} onChange={(v) => setLow(v === "low", true)} />)}
      </section>
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
