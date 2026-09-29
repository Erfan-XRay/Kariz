import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, api, setCsrf } from "./api";
import type { ServerInfo, SessionInfo } from "./api";
import { Login, tokenIn } from "./Login";
import { MapPage, ServersPage, SettingsPage } from "./pages";
import { LogsPage } from "./Logs";
import { NetworksPage } from "./Networks";
import { Shell } from "./Shell";
import { TunnelsPage } from "./Tunnels";
import { UpdateDialog, UpdatePill, UpdatesSection, useUpdate } from "./Update";
import type { PageId } from "./Shell";
import { useApp } from "./store";

type Screen = "boot" | "login" | "app";

function Boot() {
  const { t } = useApp();
  return (
    <section id="boot" className="screen is-on" aria-label="Loading">
      <div className="boot-inner">
        <svg className="boot-logo" viewBox="0 0 256 256" aria-hidden="true">
          <defs>
            <linearGradient id="bw" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0" stopColor="#1fb5a8" />
              <stop offset=".6" stopColor="#34d0c3" />
              <stop offset="1" stopColor="#7ef0e4" />
            </linearGradient>
          </defs>
          <rect className="draw" pathLength={1} x="4" y="4" width="248" height="248" rx="54" stroke="var(--line-strong)" strokeWidth="2" />
          <path className="draw d2" pathLength={1} d="M0 96 H256" stroke="#e9c46a" strokeWidth="4" />
          <g stroke="#e9c46a" strokeLinecap="round" strokeWidth="5" opacity=".6">
            <path className="draw d3" pathLength={1} d="M58 96 V146" />
            <path className="draw d3" pathLength={1} d="M104 96 V154" />
            <path className="draw d4" pathLength={1} d="M150 96 V162" />
          </g>
          <g className="fillin" fill="#e9c46a">
            <path d="M46 96 a12 9 0 0 1 24 0 Z" />
            <path d="M92 96 a12 9 0 0 1 24 0 Z" />
            <path d="M138 96 a12 9 0 0 1 24 0 Z" />
            <circle cx="44" cy="44" r="2" opacity=".8" />
            <circle cx="198" cy="34" r="1.6" opacity=".6" />
            <circle cx="150" cy="58" r="1.2" opacity=".5" />
          </g>
          <path className="fillin" d="M26 142 L186 172" stroke="#06121f" strokeWidth="26" strokeLinecap="round" />
          <path className="water" pathLength={1} d="M26 142 L186 172" stroke="url(#bw)" strokeWidth="16" strokeLinecap="round" fill="none" />
          <path className="arrow" d="M190 154 L226 176 L190 198 Z" fill="#7ef0e4" stroke="#7ef0e4" strokeWidth="6" strokeLinejoin="round" />
        </svg>
        <div className="boot-word">
          Kariz<small>{t("boot.sub")}</small>
        </div>
      </div>
    </section>
  );
}

export function App() {
  const { t, num, low, toast } = useApp();
  const [screen, setScreen] = useState<Screen>("boot");
  const [info, setInfo] = useState<SessionInfo | null>(null);
  const [page, setPage] = useState<PageId>("map");
  const [servers, setServers] = useState<ServerInfo[]>([]);
  const [agentsOn, setAgentsOn] = useState(false);
  const [reload, setReload] = useState(0);
  const [updating, setUpdating] = useState(false);
  const update = useUpdate(screen === "app");
  const [rising, setRising] = useState(false);
  const [loginKey, setLoginKey] = useState(0);
  const autoLink = useRef<string | null>(null);
  const lowRef = useRef(low);
  lowRef.current = low;

  // A login link in the address (`#t=...`) is taken out of it at once: it must not stay
  // in the history, and it works only once anyway.
  useEffect(() => {
    autoLink.current = tokenIn(location.hash);
    if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  }, []);

  // The logo draws itself while the session is looked up.
  useEffect(() => {
    let cancelled = false;
    const wait = new Promise((r) => setTimeout(r, lowRef.current ? 700 : 2300));
    const ask = api.session().catch(() => ({ authenticated: false, has_password: false }) as SessionInfo);
    void Promise.all([wait, ask]).then(([, s]) => {
      if (cancelled) return;
      setInfo(s);
      if (s.authenticated && !autoLink.current) {
        setCsrf(s.csrf);
        setRising(true);
        setScreen("app");
      } else setScreen("login");
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const signOut = useCallback(() => {
    setCsrf(undefined);
    setServers([]);
    void api.session().then(setInfo).catch(() => {});
    setLoginKey((k) => k + 1);
    setScreen("login");
  }, []);

  // The servers, kept fresh while the dashboard is open. A session that ended (a revoked
  // one, a new password elsewhere) takes the panel back to the sign-in page.
  useEffect(() => {
    if (screen !== "app") return;
    let alive = true;
    const load = () =>
      api
        .servers()
        .then((r) => {
          if (!alive) return;
          setServers(r.servers);
          setAgentsOn(r.agents);
        })
        .catch((e) => {
          if (e instanceof ApiError && e.status === 401) signOut();
        });
    void load();
    const id = setInterval(load, 2500);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [screen, signOut, reload]);

  const subtitle =
    page === "map"
      ? t("page.mapSub", { s: num(servers.length), t: num(new Set(servers.flatMap((x) => x.tunnels.map((y) => y.name))).size) })
      : page === "servers"
        ? t("srv.sub", { n: num(servers.length) })
        : page === "tunnels"
          ? t("tun.sub", { n: num(new Set(servers.flatMap((x) => x.tunnels.map((y) => y.name))).size) })
          : page === "settings"
          ? t("set.sub")
          : "";

  return (
    <>
      {screen === "boot" && <Boot />}
      {screen === "login" && info && (
        <Login
          key={loginKey}
          hasPassword={info.has_password}
          autoLink={autoLink.current}
          onSignedIn={(csrf, viaLink) => {
            setCsrf(csrf);
            autoLink.current = null;
            void api.session().then(setInfo).catch(() => {});
            setRising(true);
            setPage("map");
            setScreen("app");
            if (viaLink) setTimeout(() => toast(t("welcome")), 900);
          }}
        />
      )}
      {screen === "app" && (
        <Shell
          page={page}
          setPage={setPage}
          subtitle={subtitle}
          rising={rising}
          onLogout={() => {
            void api.logout().finally(signOut);
          }}
          onMakeLink={() => setPage("settings")}
          notice={<UpdatePill status={update.status} onOpen={() => setUpdating(true)} />}
        >
          {page === "map" && <MapPage servers={servers} />}
          {page === "servers" && <ServersPage servers={servers} agentsOn={agentsOn} onChanged={() => setReload((n) => n + 1)} />}
          {page === "tunnels" && <TunnelsPage servers={servers} onChanged={() => setReload((n) => n + 1)} />}
          {page === "networks" && <NetworksPage servers={servers} onChanged={() => setReload((n) => n + 1)} />}
          {page === "logs" && <LogsPage servers={servers} />}
          {page === "settings" && (
            <SettingsPage
              hasPassword={info?.has_password ?? false}
              onPasswordSet={() => setInfo((i) => (i ? { ...i, has_password: true } : i))}
              updates={<UpdatesSection status={update.status} reload={update.reload} />}
            />
          )}
          {updating && update.status && <UpdateDialog status={update.status} onClose={() => setUpdating(false)} />}
        </Shell>
      )}
    </>
  );
}
