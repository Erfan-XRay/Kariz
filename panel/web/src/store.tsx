import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { TABLES, localDigits } from "./i18n";
import type { Digits, Lang } from "./i18n";

export type Theme = "night" | "dawn";

export type ToastKind = "info" | "ok" | "err";

interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
  leaving: boolean;
}

interface Ctx {
  lang: Lang;
  setLang: (l: Lang) => void;
  theme: Theme;
  setTheme: (t: Theme) => void;
  low: boolean;
  setLow: (on: boolean, announce?: boolean) => void;
  digits: Digits;
  setDigits: (d: Digits) => void;
  /** A string of the current language, with `{name}` values filled in. */
  t: (key: string, vars?: Record<string, string | number>) => string;
  /** A number in the current digits, with thousands separators. */
  num: (n: number, decimals?: number) => string;
  /** Digits of a ready string. */
  digitsOf: (s: string) => string;
  /** A short note in the corner: `ok` for something done, `err` for something that failed. */
  toast: (text: string, kind?: ToastKind) => void;
  dismiss: (id: number) => void;
  toasts: Toast[];
}

const AppCtx = createContext<Ctx | null>(null);

export function useApp(): Ctx {
  const ctx = useContext(AppCtx);
  if (!ctx) throw new Error("useApp outside its provider");
  return ctx;
}

function load<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (allowed as readonly string[]).includes(v)) return v as T;
  } catch {
    // storage may be blocked; the app works without it
  }
  return fallback;
}

function save(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(() => load("kariz.lang", ["fa", "en"], "fa"));
  const [theme, setThemeState] = useState<Theme>(() => load("kariz.theme", ["night", "dawn"], "night"));
  const [digits, setDigitsState] = useState<Digits>(() => load("kariz.digits", ["fa", "latin"], "fa"));
  const [low, setLowState] = useState<boolean>(
    () => load("kariz.low", ["0", "1"], matchMedia("(prefers-reduced-motion: reduce)").matches ? "1" : "0") === "1",
  );
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextToast = useRef(1);

  // The page's language, direction and theme follow the settings before anything draws.
  useEffect(() => {
    const html = document.documentElement;
    html.lang = lang;
    html.dir = lang === "fa" ? "rtl" : "ltr";
    html.dataset.theme = theme;
  }, [lang, theme]);

  const t = useCallback(
    (key: string, vars?: Record<string, string | number>) => {
      let text = TABLES[lang][key] ?? TABLES.en[key] ?? key;
      if (vars) for (const [k, v] of Object.entries(vars)) text = text.replaceAll(`{${k}}`, String(v));
      return text;
    },
    [lang],
  );

  const dismiss = useCallback((id: number) => {
    setToasts((list) => list.map((x) => (x.id === id ? { ...x, leaving: true } : x)));
    setTimeout(() => setToasts((list) => list.filter((x) => x.id !== id)), 300);
  }, []);

  const toast = useCallback(
    (text: string, kind: ToastKind = "info") => {
      const id = nextToast.current++;
      // at most three at a time: the oldest goes first
      setToasts((list) => [...list.slice(-2), { id, text, kind, leaving: false }]);
      setTimeout(() => dismiss(id), kind === "err" ? 6000 : 3600);
    },
    [dismiss],
  );

  const value = useMemo<Ctx>(() => {
    const digitsOf = (s: string) => localDigits(s, lang, digits);
    const num = (n: number, decimals = 0) => {
      const [i, f] = Math.abs(n).toFixed(decimals).split(".");
      const grouped = i.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
      return digitsOf((n < 0 ? "-" : "") + grouped + (f ? `.${f}` : ""));
    };
    return {
      lang,
      setLang: (l) => {
        save("kariz.lang", l);
        setLangState(l);
      },
      theme,
      setTheme: (th) => {
        save("kariz.theme", th);
        setThemeState(th);
      },
      low,
      setLow: (on, announce) => {
        save("kariz.low", on ? "1" : "0");
        setLowState(on);
        if (announce) toast(t(on ? "toast.low.on" : "toast.low.off"));
      },
      digits,
      setDigits: (d) => {
        save("kariz.digits", d);
        setDigitsState(d);
      },
      t,
      num,
      digitsOf,
      toast,
      dismiss,
      toasts,
    };
  }, [lang, theme, low, digits, t, toast, dismiss, toasts]);

  return <AppCtx.Provider value={value}>{children}</AppCtx.Provider>;
}
