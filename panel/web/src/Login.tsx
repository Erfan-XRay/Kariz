import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { ApiError, api } from "./api";
import { paletteNow } from "./draw";
import { startLoginScene } from "./scene-login";
import type { LoginScene } from "./scene-login";
import { useApp } from "./store";
import { Icon } from "./ui";
import logo from "./logo.svg";

/** The token in what the user pasted: a whole link, or just its 64 characters. */
export function tokenIn(text: string): string | null {
  const m = text.match(/#t=([0-9a-f]{64})/i) ?? text.match(/\b([0-9a-f]{64})\b/i);
  return m ? m[1].toLowerCase() : null;
}

type T = (key: string, vars?: Record<string, string | number>) => string;

/** What to tell the user when signing in failed. */
export function loginError(e: unknown, link: boolean, t: T, num: (n: number) => string): string {
  if (e instanceof ApiError && e.code === "locked") {
    const secs = Number(e.body.retry_after ?? 900);
    return t("login.locked", { m: num(Math.max(1, Math.ceil(secs / 60))) });
  }
  if (e instanceof ApiError && e.code === "wrong") return t(link ? "login.wrongLink" : "login.wrong", { n: num(Number(e.body.tries_left ?? 0)) });
  return t("login.error");
}

export function Login({
  hasPassword,
  initialError = "",
  onSignedIn,
}: {
  hasPassword: boolean;
  /** Why a link in the address did not sign in, if one was there. */
  initialError?: string;
  onSignedIn: (csrf: string) => void;
}) {
  const { t, lang, setLang, theme, setTheme, low, num } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const scene = useRef<LoginScene | null>(null);
  const lowRef = useRef(low);
  const [mode, setMode] = useState<"password" | "link">(hasPassword && !initialError ? "password" : "link");
  const [password, setPassword] = useState("");
  const [shown, setShown] = useState(false);
  const [caps, setCaps] = useState(false);
  const [linkText, setLinkText] = useState("");
  const [error, setError] = useState(initialError);
  const [busy, setBusy] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [shake, setShake] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    lowRef.current = low;
  }, [low]);

  useEffect(() => {
    if (!canvas.current) return;
    const s = startLoginScene(canvas.current, {
      palette: paletteNow,
      low: () => lowRef.current,
      rtl: () => document.documentElement.dir === "rtl",
    });
    scene.current = s;
    return () => s.stop();
  }, []);

  // The server said yes. Before the dashboard opens, check that the browser kept the
  // session cookie: some (Safari over a self-signed certificate) drop it silently, and the
  // panel would then send the user straight back here with no word why.
  const succeed = useCallback(
    async (csrf: string) => {
      const kept = await api.session().then((s) => s.authenticated, () => false);
      if (!kept) {
        setBusy(false);
        scene.current?.ripple();
        setShake((n) => n + 1);
        setError(t("login.noCookie"));
        return;
      }
      setLeaving(true);
      scene.current?.descend(() => onSignedIn(csrf));
    },
    [onSignedIn, t],
  );

  const fail = (message: string) => {
    setBusy(false);
    scene.current?.ripple();
    setShake((n) => n + 1);
    setError(message);
    requestAnimationFrame(() => input.current?.focus());
  };

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (busy || leaving) return;
    if (mode === "link") {
      const token = tokenIn(linkText);
      if (!token) return fail(t(linkText.trim() ? "login.notLink" : "login.emptyLink"));
      setBusy(true);
      setError("");
      try {
        const { csrf } = await api.loginWithLink(token);
        await succeed(csrf);
      } catch (e) {
        fail(loginError(e, true, t, num));
      }
      return;
    }
    if (!password) return fail(t("login.empty"));
    setBusy(true);
    setError("");
    try {
      const { csrf } = await api.login(password);
      await succeed(csrf);
    } catch (e) {
      fail(loginError(e, false, t, num));
    }
  };

  const switchTo = (m: "password" | "link") => {
    setMode(m);
    setError("");
    requestAnimationFrame(() => input.current?.focus());
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => setCaps(e.getModifierState?.("CapsLock") ?? false);
  const hint = mode === "link" ? (hasPassword ? t("login.linkHint") : t("login.noPassword")) : "";

  return (
    <section id="login" className="screen is-on" aria-labelledby="login-title">
      <canvas id="login-scene" ref={canvas} aria-hidden="true" />
      <div className="login-top">
        <div className="brand">
          <img src={logo} alt="" />
          <span>Kariz</span>
        </div>
        <div className="chip-group">
          <button className="chip" type="button" onClick={() => setLang(lang === "fa" ? "en" : "fa")}>
            {lang === "fa" ? "English" : "فارسی"}
          </button>
          <button className="chip" type="button" onClick={() => setTheme(theme === "night" ? "dawn" : "night")}>
            <Icon name={theme === "night" ? "sun" : "moon"} size={16} />
            {theme === "night" ? t("theme.dawn") : t("theme.night")}
          </button>
        </div>
      </div>

      <form className={`login-form ${leaving ? "is-leaving" : ""}`} onSubmit={submit} noValidate aria-busy={busy}>
        <div className="login-head">
          <h1 className="login-title" id="login-title">
            {t("login.title")}
          </h1>
          <p className="login-sub">{t("login.sub")}</p>
        </div>

        {hasPassword && (
          <div className="login-tabs" role="group" aria-label={t("login.how")}>
            <button type="button" aria-pressed={mode === "password"} onClick={() => switchTo("password")}>
              <Icon name="key" size={16} />
              {t("login.tabPassword")}
            </button>
            <button type="button" aria-pressed={mode === "link"} onClick={() => switchTo("link")}>
              <Icon name="link" size={16} />
              {t("login.tabLink")}
            </button>
          </div>
        )}

        <div key={`${mode}-${shake}`} className={`login-field ${shake ? "shake" : ""}`}>
          <label className="field-label" htmlFor="pw">
            {mode === "password" ? t("login.password") : t("login.linkLabel")}
          </label>
          <div className="login-input" dir={mode === "link" ? "ltr" : undefined}>
            <Icon name={mode === "password" ? "key" : "link"} size={18} />
            {mode === "password" ? (
              <input
                ref={input}
                className="input mono"
                id="pw"
                type={shown ? "text" : "password"}
                autoComplete="current-password"
                autoFocus
                value={password}
                aria-invalid={!!error}
                aria-describedby="pw-err pw-hint"
                onKeyUp={onKey}
                onKeyDown={onKey}
                onChange={(e) => {
                  setPassword(e.target.value);
                  setError("");
                }}
              />
            ) : (
              <input
                ref={input}
                className="input mono"
                id="pw"
                type="text"
                dir="ltr"
                autoComplete="off"
                autoFocus
                spellCheck={false}
                placeholder="https://…/#t=…"
                value={linkText}
                aria-invalid={!!error}
                aria-describedby="pw-err pw-hint"
                onChange={(e) => {
                  setLinkText(e.target.value);
                  setError("");
                }}
              />
            )}
            {mode === "password" && (
              <button className="icon-btn" type="button" aria-label={t(shown ? "login.hide" : "login.show")} aria-pressed={shown} onClick={() => setShown((v) => !v)}>
                <Icon name={shown ? "eyeOff" : "eye"} size={18} />
              </button>
            )}
          </div>
          <p className="field-error" id="pw-err" role="alert">
            {error}
          </p>
          <p className="field-hint" id="pw-hint">
            {!error && mode === "password" && caps ? <span className="caps">{t("login.caps")}</span> : !error ? hint : ""}
          </p>
        </div>

        <button className={`btn btn-primary btn-block ${busy ? "is-busy" : ""}`} type="submit" disabled={busy || leaving}>
          <span className="shine" />
          {busy && <span className="spinner" aria-hidden="true" />}
          <span>{busy ? t(mode === "link" ? "login.checking" : "login.signing") : t("login.enter")}</span>
        </button>

        <p className="login-foot">
          <span>{t("login.foot")}</span>
          <code dir="ltr">kariz-manager panel link</code>
        </p>
      </form>
    </section>
  );
}
