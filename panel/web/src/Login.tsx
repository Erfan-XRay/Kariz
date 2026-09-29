import { useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
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

export function Login({
  hasPassword,
  autoLink,
  onSignedIn,
}: {
  hasPassword: boolean;
  /** A token from the address (`#t=...`): signs in by itself. */
  autoLink: string | null;
  onSignedIn: (csrf: string, viaLink: boolean) => void;
}) {
  const { t, lang, setLang, theme, setTheme, low, num } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const scene = useRef<LoginScene | null>(null);
  const lowRef = useRef(low);
  const [mode, setMode] = useState<"password" | "link">(hasPassword ? "password" : "link");
  const [password, setPassword] = useState("");
  const [shown, setShown] = useState(false);
  const [linkText, setLinkText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [shake, setShake] = useState(0);
  const auto = useRef(autoLink);

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

  const succeed = useCallback(
    (csrf: string, viaLink: boolean) => {
      setLeaving(true);
      scene.current?.descend(() => onSignedIn(csrf, viaLink));
    },
    [onSignedIn],
  );

  const fail = useCallback(
    (e: unknown, link: boolean) => {
      setBusy(false);
      scene.current?.ripple();
      setShake((n) => n + 1);
      if (e instanceof ApiError && e.code === "locked") {
        const secs = Number(e.body.retry_after ?? 900);
        setError(t("login.locked", { m: num(Math.max(1, Math.ceil(secs / 60))) }));
      } else if (e instanceof ApiError && e.code === "wrong") {
        setError(t(link ? "login.wrongLink" : "login.wrong", { n: num(Number(e.body.tries_left ?? 0)) }));
      } else {
        setError(t("login.error"));
      }
    },
    [t, num],
  );

  const signInWithLink = useCallback(
    async (token: string) => {
      setBusy(true);
      setError("");
      try {
        const { csrf } = await api.loginWithLink(token);
        succeed(csrf, true);
      } catch (e) {
        fail(e, true);
      }
    },
    [succeed, fail],
  );

  // A link in the address signs in by itself.
  useEffect(() => {
    if (auto.current) {
      const token = auto.current;
      auto.current = null;
      void signInWithLink(token);
    }
  }, [signInWithLink]);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (busy || leaving) return;
    if (mode === "link") {
      const token = tokenIn(linkText);
      if (!token) return fail(new ApiError(401, { error: "wrong", tries_left: 5 }), true);
      return signInWithLink(token);
    }
    if (!password) {
      setError(t("login.empty"));
      setShake((n) => n + 1);
      return;
    }
    setBusy(true);
    setError("");
    try {
      const { csrf } = await api.login(password);
      succeed(csrf, false);
    } catch (e) {
      fail(e, false);
    }
  };

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
            {theme === "night" ? t("theme.dawn") : t("theme.night")}
          </button>
        </div>
      </div>

      <form className={`login-form ${leaving ? "is-leaving" : ""} ${busy && mode === "link" ? "is-busy" : ""}`} onSubmit={submit} noValidate>
        <h1 className="login-title" id="login-title">
          {t("login.title")}
        </h1>
        <p className="login-sub">{t("login.sub")}</p>

        <div className="login-fields">
          <div key={shake} className={`well-field ${shake ? "shake" : ""}`}>
            {mode === "password" ? (
              <>
                <label className="field-label" htmlFor="pw">
                  {t("login.password")}
                </label>
                <input
                  className="input mono"
                  id="pw"
                  type={shown ? "text" : "password"}
                  autoComplete="current-password"
                  autoFocus
                  value={password}
                  aria-invalid={!!error}
                  aria-describedby="pw-err"
                  onChange={(e) => {
                    setPassword(e.target.value);
                    setError("");
                  }}
                />
                <button className="icon-btn" type="button" aria-label={t("login.show")} onClick={() => setShown((v) => !v)}>
                  <Icon name="eye" />
                </button>
              </>
            ) : (
              <>
                <label className="field-label" htmlFor="pw">
                  {t("login.linkLabel")}
                </label>
                <input
                  className="input mono"
                  id="pw"
                  type="text"
                  dir="ltr"
                  autoComplete="off"
                  autoFocus
                  spellCheck={false}
                  value={linkText}
                  aria-invalid={!!error}
                  aria-describedby="pw-err"
                  onChange={(e) => {
                    setLinkText(e.target.value);
                    setError("");
                  }}
                />
              </>
            )}
          </div>
          <p className="field-error" id="pw-err" role="alert">
            {error || (!hasPassword && mode === "link" ? t("login.noPassword") : "")}
          </p>
          <div style={{ display: "grid", gap: "var(--sp-3)" }}>
            <button className="btn btn-primary" type="submit" disabled={busy || leaving}>
              <span className="shine" />
              <span>{t("login.enter")}</span>
            </button>
            {hasPassword && (
              <>
                <div className="divider">{t("login.or")}</div>
                <button
                  className="btn btn-ghost"
                  type="button"
                  onClick={() => {
                    setMode(mode === "link" ? "password" : "link");
                    setError("");
                  }}
                >
                  {mode === "link" ? t("login.usePassword") : t("login.link")}
                </button>
              </>
            )}
            <p className="login-foot">
              <span>{t("login.foot")}</span> <span className="mono" dir="ltr">kariz-panel login-link</span>
            </p>
          </div>
        </div>

        <div className="login-busy" aria-live="polite">
          <svg className="qloader" viewBox="0 0 180 90" aria-hidden="true">
            <path className="bed" d="M30 58 L158 76" />
            <path className="flow" pathLength={1} d="M30 58 L158 76" />
            <path className="shaft" d="M38 26 V58 M84 26 V64 M130 26 V70" />
            <path className="ground" d="M8 26 H172" />
            <path className="mound" d="M30 26 a8 6 0 0 1 16 0Z M76 26 a8 6 0 0 1 16 0Z M122 26 a8 6 0 0 1 16 0Z" />
            <circle className="drop" cx="38" cy="26" r="4" />
            <path className="out" d="M164 68 L178 77 L164 86 Z" />
          </svg>
          <span>{t("login.checking")}</span>
        </div>
      </form>
    </section>
  );
}
