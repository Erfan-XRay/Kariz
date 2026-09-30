import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useApp } from "./store";
import { Icon, useFocusTrap } from "./ui";
import logo from "./logo.svg";

export type PageId = "map" | "servers" | "tunnels" | "networks" | "logs" | "settings";
export const PAGES: PageId[] = ["map", "servers", "tunnels", "networks", "logs", "settings"];

interface Command {
  label: string;
  run: () => void;
  hint?: string;
}

export function Shell({
  page,
  setPage,
  subtitle,
  rising,
  onLogout,
  onMakeLink,
  notice,
  children,
}: {
  page: PageId;
  setPage: (p: PageId) => void;
  subtitle: string;
  rising: boolean;
  onLogout: () => void;
  onMakeLink: () => void;
  /** Something for the top bar, such as the notice of a newer release. */
  notice?: ReactNode;
  children: ReactNode;
}) {
  const { t, lang, setLang, theme, setTheme, low, setLow, toasts } = useApp();
  const [palette, setPalette] = useState(false);
  const main = useRef<HTMLElement>(null);

  useEffect(() => {
    main.current?.scrollTo({ top: 0 });
  }, [page]);

  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((v) => !v);
      }
    };
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, []);

  const commands = useMemo<Command[]>(
    () => [
      ...PAGES.map((p) => ({ label: `${t("pal.goto")} ${t(`page.${p}`)}`, run: () => setPage(p) })),
      { label: t("pal.link"), run: onMakeLink },
      { label: t("pal.theme"), run: () => setTheme(theme === "night" ? "dawn" : "night") },
      { label: t("pal.lang"), run: () => setLang(lang === "fa" ? "en" : "fa") },
      { label: t("pal.low"), hint: low ? "✓" : "", run: () => setLow(!low, true) },
      { label: t("pal.logout"), run: onLogout },
    ],
    [t, theme, lang, low, setPage, setTheme, setLang, setLow, onLogout, onMakeLink],
  );

  return (
    <section id="app" className={`screen is-on ${rising ? "is-rising" : ""}`} aria-label="Kariz panel">
      <nav className="rail" aria-label="Main">
        <div className="brand">
          <img src={logo} alt="Kariz" />
        </div>
        {PAGES.filter((p) => p !== "settings").map((p) => (
          <button key={p} className="rail-item" type="button" aria-current={page === p ? "page" : undefined} onClick={() => setPage(p)}>
            <Icon name={p} size={24} />
            <span>{t(`nav.${p}`)}</span>
          </button>
        ))}
        <div className="rail-spacer" />
        <button className="rail-item" type="button" aria-current={page === "settings" ? "page" : undefined} onClick={() => setPage("settings")}>
          <Icon name="settings" size={24} />
          <span>{t("nav.settings")}</span>
        </button>
        <button className="rail-item rail-extra" type="button" onClick={onLogout}>
          <Icon name="exit" size={24} />
          <span>{t("nav.logout")}</span>
        </button>
      </nav>

      <header className="sky">
        <h1 className="page-title">
          <span>{t(`page.${page}`)}</span>
          <small>{subtitle}</small>
        </h1>
        <div className="sky-spacer" />
        {notice}
        <button className="search-btn" type="button" onClick={() => setPalette(true)} aria-label={t("search")}>
          <Icon name="search" size={18} />
          <span className="label">{t("search")}</span>
          <kbd>Ctrl K</kbd>
        </button>
        <div className="sky-actions">
          <button className="square-btn" type="button" onClick={() => setLang(lang === "fa" ? "en" : "fa")} aria-label="Language">
            {lang === "fa" ? "EN" : "فا"}
          </button>
          <button className="square-btn" type="button" onClick={() => setTheme(theme === "night" ? "dawn" : "night")} aria-label="Theme">
            <Icon name={theme === "night" ? "sun" : "moon"} />
          </button>
          <button className="square-btn" type="button" onClick={() => setLow(!low, true)} aria-label="Low power" aria-pressed={low}>
            <Icon name="leaf" />
          </button>
        </div>
      </header>

      <main className="main" ref={main} tabIndex={0} aria-label={t(`page.${page}`)}>
        {children}
      </main>

      {palette && <Palette commands={commands} onClose={() => setPalette(false)} />}
      <div className="toasts" aria-live="polite">
        {toasts.map((x) => (
          <div key={x.id} className={`toast ${x.leaving ? "is-leaving" : ""}`}>
            {x.text}
          </div>
        ))}
      </div>
    </section>
  );
}

function Palette({ commands, onClose }: { commands: Command[]; onClose: () => void }) {
  const { t } = useApp();
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);
  const [on, setOn] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useFocusTrap(box);
  const items = commands.filter((c) => c.label.toLowerCase().includes(query.trim().toLowerCase()));

  useEffect(() => {
    const id = requestAnimationFrame(() => setOn(true));
    input.current?.focus();
    return () => cancelAnimationFrame(id);
  }, []);

  const run = (c: Command | undefined) => {
    onClose();
    c?.run();
  };

  return (
    <div ref={box} className={`palette-backdrop ${on ? "is-on" : ""}`} role="dialog" aria-modal="true" aria-label="Commands" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette">
        <input
          ref={input}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls="pal-list"
          aria-autocomplete="list"
          aria-activedescendant={items.length ? `pal-${sel}` : undefined}
          aria-label={t("pal.title")}
          autoComplete="off"
          placeholder={t("pal.placeholder")}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSel((sel + 1) % Math.max(1, items.length));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSel((sel - 1 + items.length) % Math.max(1, items.length));
            } else if (e.key === "Enter") run(items[sel]);
            else if (e.key === "Escape") onClose();
          }}
        />
        <ul role="listbox" id="pal-list" aria-label={t("pal.title")} tabIndex={-1}>
          {items.map((c, i) => (
            <li key={c.label} id={`pal-${i}`} role="option" aria-selected={i === sel} onMouseMove={() => setSel(i)} onClick={() => run(c)}>
              <span>{c.label}</span>
              <span className="hint">{c.hint}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
