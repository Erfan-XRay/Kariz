import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { FA_DIGITS } from "./i18n";
import { useApp } from "./store";

const ICONS: Record<string, ReactNode> = {
  map: (
    <>
      <path d="M2 9h20" />
      <path d="M6 9v6M12 9v8M18 9v6" />
      <path d="M4 15.5c4 1.2 12 2.4 16 2" opacity=".6" />
      <path d="M4.5 9a1.5 1.2 0 0 1 3 0M10.5 9a1.5 1.2 0 0 1 3 0M16.5 9a1.5 1.2 0 0 1 3 0" />
    </>
  ),
  servers: (
    <>
      <rect x="3.5" y="4" width="17" height="6.5" rx="1.5" />
      <rect x="3.5" y="13.5" width="17" height="6.5" rx="1.5" />
      <path d="M7 7.25h.01M7 16.75h.01M11 7.25h6M11 16.75h6" />
    </>
  ),
  tunnels: (
    <>
      <path d="M3 17c3-6 15-6 18 0" />
      <path d="M6.5 17c2-3.2 9-3.2 11 0" />
      <path d="M3 17h3.5M17.5 17H21" />
    </>
  ),
  logs: <path d="M5 5h14M5 9.5h9M5 14h14M5 18.5h7" />,
  settings: (
    <>
      <path d="M4 7h10M18 7h2M4 17h2M10 17h10" />
      <circle cx="16" cy="7" r="2" />
      <circle cx="8" cy="17" r="2" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4 4" />
    </>
  ),
  moon: <path d="M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10Z" />,
  sun: (
    <>
      <path d="M3 16h18" />
      <path d="M7 16a5 5 0 0 1 10 0" />
      <path d="M12 5v2.5M5.5 8.5l1.6 1.6M18.5 8.5l-1.6 1.6" />
    </>
  ),
  leaf: <path d="M13 3 5 13.5h6L10 21l8-10.5h-6L13 3Z" />,
  eye: (
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  arrow: <path d="M4 12h15M14 7l5 5-5 5" />,
  exit: (
    <>
      <path d="M14 4h4.5A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5H14" />
      <path d="M10 8l-4 4 4 4M6 12h10" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="12" rx="2" />
      <path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8" />
    </>
  ),
  check: <path d="m5 12.5 4.5 4.5L19 7.5" />,
  x: <path d="M6 6l12 12M18 6 6 18" />,
  key: (
    <>
      <circle cx="8" cy="15" r="4" />
      <path d="m11 12 8-8M16 7l2.5 2.5M14 9l2 2" />
    </>
  ),
};

export function Icon({ name, size = 20 }: { name: keyof typeof ICONS | string; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {ICONS[name]}
    </svg>
  );
}

/** A number whose digits roll to their value, like an odometer. `value` is a ready string. */
export function Odo({ value }: { value: string }) {
  const { digitsOf } = useApp();
  const [shown, setShown] = useState<string | null>(null);
  // Starts at zero and rolls on the next frame; later changes roll at once.
  useEffect(() => {
    const id = requestAnimationFrame(() => requestAnimationFrame(() => setShown(value)));
    return () => cancelAnimationFrame(id);
  }, [value]);
  const digitAt = (ch: string) => {
    const fa = FA_DIGITS.indexOf(ch);
    return fa >= 0 ? fa : /[0-9]/.test(ch) ? Number(ch) : -1;
  };
  const glyphs = Array.from({ length: 10 }, (_, d) => digitsOf(String(d)));
  const target = shown ?? value;
  return (
    <span className="odo" aria-label={value}>
      {[...value].map((ch, i) => {
        const d = digitAt(ch);
        if (d < 0) return <span key={i}>{ch}</span>;
        const at = shown === null ? 0 : digitAt(target[i] ?? ch);
        return (
          <span className="odo-d" key={i}>
            <span className="odo-c" style={{ transform: `translateY(${-Math.max(0, at) * 10}%)` }}>
              {glyphs.map((g, k) => (
                <span key={k}>{g}</span>
              ))}
            </span>
          </span>
        );
      })}
    </span>
  );
}

export function Dialog({
  title,
  onClose,
  children,
  footer,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const { t } = useApp();
  const ref = useRef<HTMLDivElement>(null);
  const [on, setOn] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setOn(true));
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    addEventListener("keydown", key);
    ref.current?.querySelector<HTMLElement>("input, button:not(.x-btn)")?.focus();
    return () => {
      cancelAnimationFrame(id);
      removeEventListener("keydown", key);
    };
  }, [onClose]);
  return (
    <div className={`dialog-backdrop ${on ? "is-on" : ""}`} role="dialog" aria-modal="true" aria-label={title} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" ref={ref}>
        <header>
          <h2>{title}</h2>
          <button className="x-btn" type="button" onClick={onClose} aria-label={t("close")}>
            <Icon name="x" />
          </button>
        </header>
        <div className="body">{children}</div>
        {footer && <footer>{footer}</footer>}
      </div>
    </div>
  );
}

/** A row of choices, one pressed. */
export function Seg<T extends string>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="seg" role="group">
      {options.map(([v, label]) => (
        <button key={v} type="button" aria-pressed={value === v} onClick={() => onChange(v)}>
          {label}
        </button>
      ))}
    </div>
  );
}

/** A block of code with a copy button. */
export function CodeBlock({ text }: { text: string }) {
  const { t } = useApp();
  const [done, setDone] = useState(false);
  return (
    <pre className="code">
      {text}
      <button
        className={`copy-btn ${done ? "done" : ""}`}
        type="button"
        aria-label={t("copied")}
        onClick={() => {
          navigator.clipboard?.writeText(text).catch(() => {});
          setDone(true);
          setTimeout(() => setDone(false), 1600);
        }}
      >
        <Icon name={done ? "check" : "copy"} size={18} />
      </button>
    </pre>
  );
}

/** "12 s ago", "3 h ago". */
export function useAgo() {
  const { t, num } = useApp();
  return (secs: number) => {
    if (secs < 60) return t("ago.s", { n: num(Math.max(0, Math.round(secs))) });
    if (secs < 3600) return t("ago.m", { n: num(Math.floor(secs / 60)) });
    if (secs < 86400) return t("ago.h", { n: num(Math.floor(secs / 3600)) });
    return t("ago.d", { n: num(Math.floor(secs / 86400)) });
  };
}
