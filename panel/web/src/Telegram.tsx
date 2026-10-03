import { useCallback, useEffect, useState } from "react";
import { ApiError, api } from "./api";
import type { TelegramChange, TelegramView } from "./api";
import { useApp } from "./store";
import { Card, Seg } from "./ui";

/** Settings > Telegram: alerts when a server or tunnel goes down, and a bot to ask for the status. */
export function TelegramSection() {
  const { t, toast } = useApp();
  const [view, setView] = useState<TelegramView | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [adv, setAdv] = useState({ proxy: "", via: "", base: "" });
  const [graces, setGraces] = useState({ server: "", tunnel: "" });

  const adopt = useCallback((v: TelegramView) => {
    setView(v);
    setAdv({ proxy: v.proxy, via: v.connect_via, base: v.api_base });
    setGraces({ server: String(v.server_grace), tunnel: String(v.tunnel_grace) });
  }, []);

  useEffect(() => {
    let alive = true;
    api
      .telegram()
      .then((v) => alive && adopt(v))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [adopt]);

  // While a connect code is showing, look for the chat that uses it.
  const waiting = !!view?.pair;
  useEffect(() => {
    if (!waiting) return;
    const id = window.setInterval(() => {
      api
        .telegram()
        .then((v) => setView((old) => (old && (v.chats.length !== old.chats.length || !v.pair) ? v : old)))
        .catch(() => {});
    }, 3000);
    return () => window.clearInterval(id);
  }, [waiting]);

  const errorText = (e: unknown) => (e instanceof ApiError ? (t(`tg.err.${e.code}`) !== `tg.err.${e.code}` ? t(`tg.err.${e.code}`) : e.code) : String(e));

  const change = async (body: TelegramChange, done?: () => void) => {
    setBusy(true);
    try {
      adopt(await api.telegramSet(body));
      done?.();
    } catch (e) {
      toast(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const row = (title: string, text: string, control: React.ReactNode) => (
    <div className="set-row">
      <div>
        <h3>{title}</h3>
        {text && <p>{text}</p>}
      </div>
      <div className="ctl">{control}</div>
    </div>
  );
  const onOff = (value: boolean, set: (v: boolean) => void) => (
    <Seg
      value={value ? "on" : "off"}
      options={[
        ["on", t("tg.on")],
        ["off", t("tg.off")],
      ]}
      onChange={(v) => set(v === "on")}
    />
  );

  if (!view) return null;

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const r = await api.telegramTest();
      setResult(r.ok ? { ok: true, text: t("tg.testOk", { bot: r.bot ?? "" }) } : { ok: false, text: t("tg.testFail", { why: r.error ?? "" }) });
      api.telegram().then(adopt).catch(() => {});
    } catch (e) {
      setResult({ ok: false, text: t("tg.testFail", { why: errorText(e) }) });
    } finally {
      setTesting(false);
    }
  };

  const pair = async () => {
    try {
      setView(await api.telegramPair());
    } catch (e) {
      toast(errorText(e));
    }
  };

  const saveGraces = () => {
    const server = Number(graces.server);
    const tunnel = Number(graces.tunnel);
    if (!Number.isInteger(server) || !Number.isInteger(tunnel)) return toast(t("tg.err.bad_input"));
    void change({ server_grace: server, tunnel_grace: tunnel });
  };

  return (
    <Card title={t("set.telegram")} sub={t("tg.intro")}>
      {!view.token_set ? (
        row(
          t("tg.token"),
          t("tg.tokenText"),
          <form
            style={{ display: "grid", gap: "var(--sp-3)", justifyItems: "start", width: "100%" }}
            onSubmit={(e) => {
              e.preventDefault();
              void change({ token: token.trim() }, () => setToken(""));
            }}
          >
            <input
              className="text mono"
              dir="ltr"
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder="123456789:AAH…"
              aria-label={t("tg.token")}
              value={token}
              onChange={(e) => setToken(e.target.value)}
              style={{ width: "100%", maxWidth: 420 }}
            />
            <button className="btn btn-primary btn-sm" type="submit" disabled={busy || token.trim() === ""}>
              {t("tg.saveToken")}
            </button>
          </form>,
        )
      ) : (
        <>
          {row(
            t("tg.bot", { tail: view.token_tail }),
            t("tg.enableText"),
            <span style={{ display: "flex", gap: "var(--sp-3)", flexWrap: "wrap", alignItems: "center" }}>
              {onOff(view.enabled, (v) => void change({ enabled: v }))}
              <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={() => void change({ remove_token: true })}>
                {t("tg.removeBot")}
              </button>
            </span>,
          )}
          {row(
            t("tg.chats"),
            view.chats.length === 0 ? t("tg.noChats") : t("tg.chatsText"),
            <div style={{ display: "grid", gap: "var(--sp-3)", justifyItems: "start" }}>
              {view.chats.map((c) => (
                <span key={c.id} style={{ display: "flex", gap: "var(--sp-3)", alignItems: "center" }}>
                  <b>{c.name}</b>
                  <button
                    className="btn btn-ghost btn-sm"
                    type="button"
                    onClick={() =>
                      void api
                        .telegramRemoveChat(c.id)
                        .then(adopt)
                        .catch((e) => toast(errorText(e)))
                    }
                  >
                    {t("tg.disconnect")}
                  </button>
                </span>
              ))}
              {view.pair ? (
                <div className="field">
                  <span className="help">{t("tg.pairText", { m: Math.max(1, Math.round(view.pair.expires_in / 60)) })}</span>
                  <code className="mono" dir="ltr" style={{ userSelect: "all" }}>
                    /start {view.pair.code}
                  </code>
                  <span className="help">{t("tg.pairWait")}</span>
                </div>
              ) : (
                <button className="btn btn-ghost btn-sm" type="button" onClick={() => void pair()}>
                  {t("tg.connect")}
                </button>
              )}
            </div>,
          )}
          {row(
            t("tg.events"),
            "",
            <div style={{ display: "grid", gap: "var(--sp-3)", justifyItems: "start" }}>
              <span style={{ display: "flex", gap: "var(--sp-3)", alignItems: "center", flexWrap: "wrap" }}>
                {onOff(view.servers, (v) => void change({ servers: v }))}
                <span>{t("tg.servers")}</span>
              </span>
              <span style={{ display: "flex", gap: "var(--sp-3)", alignItems: "center", flexWrap: "wrap" }}>
                {onOff(view.tunnels, (v) => void change({ tunnels: v }))}
                <span>{t("tg.tunnels")}</span>
              </span>
            </div>,
          )}
          {row(
            t("tg.grace"),
            t("tg.graceText"),
            <div style={{ display: "flex", gap: "var(--sp-3)", flexWrap: "wrap", alignItems: "end" }}>
              <div className="field">
                <label htmlFor="tg-sg">{t("tg.serverGrace")}</label>
                <input className="text mono" id="tg-sg" dir="ltr" inputMode="numeric" value={graces.server} onChange={(e) => setGraces({ ...graces, server: e.target.value })} style={{ width: 110 }} />
              </div>
              <div className="field">
                <label htmlFor="tg-tg">{t("tg.tunnelGrace")}</label>
                <input className="text mono" id="tg-tg" dir="ltr" inputMode="numeric" value={graces.tunnel} onChange={(e) => setGraces({ ...graces, tunnel: e.target.value })} style={{ width: 110 }} />
              </div>
              <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={saveGraces}>
                {t("tg.save")}
              </button>
            </div>,
          )}
          {row(
            t("tg.lang"),
            "",
            <Seg
              value={view.lang}
              options={[
                ["fa", "فارسی"],
                ["en", "English"],
              ]}
              onChange={(v) => void change({ lang: v })}
            />,
          )}
          {row(
            t("tg.digest"),
            t("tg.digestText"),
            <Seg
              value={String(view.digest_hours)}
              options={[
                ["0", t("tg.off")],
                ["6", t("tg.hours", { n: 6 })],
                ["12", t("tg.hours", { n: 12 })],
                ["24", t("tg.hours", { n: 24 })],
              ]}
              onChange={(v) => void change({ digest_hours: Number(v) })}
            />,
          )}
          {row(
            t("tg.test"),
            view.muted_until ? t("tg.muted", { time: new Date(view.muted_until * 1000).toLocaleString() }) : "",
            <div style={{ display: "grid", gap: "var(--sp-3)", justifyItems: "start" }}>
              <button className="btn btn-ghost btn-sm" type="button" disabled={testing} onClick={() => void test()}>
                {t("tg.testBtn")}
              </button>
              {result && <span className={result.ok ? "ok small" : "err small"}>{result.text}</span>}
              {!result && view.last_error && <span className="err small">{t("tg.lastError", { why: view.last_error })}</span>}
            </div>,
          )}
          <details style={{ marginTop: "var(--sp-4)" }}>
            <summary>{t("tg.advanced")}</summary>
            <p className="help" style={{ margin: "var(--sp-3) 0" }}>
              {t("tg.advancedText")}
            </p>
            <div className="field">
              <label htmlFor="tg-proxy">{t("tg.proxy")}</label>
              <input className="text mono" id="tg-proxy" dir="ltr" autoComplete="off" placeholder="socks5://127.0.0.1:10808" value={adv.proxy} onChange={(e) => setAdv({ ...adv, proxy: e.target.value })} />
            </div>
            <div className="field">
              <label htmlFor="tg-via">{t("tg.via")}</label>
              <input className="text mono" id="tg-via" dir="ltr" autoComplete="off" placeholder="127.0.0.1:8443" value={adv.via} onChange={(e) => setAdv({ ...adv, via: e.target.value })} />
              <span className="help">{t("tg.viaText")}</span>
            </div>
            <div className="field">
              <label htmlFor="tg-base">{t("tg.apiBase")}</label>
              <input className="text mono" id="tg-base" dir="ltr" autoComplete="off" placeholder="https://relay.example.com" value={adv.base} onChange={(e) => setAdv({ ...adv, base: e.target.value })} />
              <span className="help">{t("tg.apiBaseText")}</span>
            </div>
            <button
              className="btn btn-ghost btn-sm"
              type="button"
              disabled={busy}
              style={{ marginTop: "var(--sp-3)" }}
              onClick={() => void change({ proxy: adv.proxy, connect_via: adv.via, api_base: adv.base }, () => toast(t("tg.saved")))}
            >
              {t("tg.save")}
            </button>
          </details>
        </>
      )}
    </Card>
  );
}
