import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import type { ServerInfo } from "./api";
import { useApp } from "./store";
import { Icon, StatePill } from "./ui";

/** The ids of `order` that are still in `items`, then any item that came since. */
export function arranged(order: string[], items: { id: string }[]): string[] {
  const known = new Set(items.map((s) => s.id));
  const kept = order.filter((id) => known.has(id));
  return [...kept, ...items.map((s) => s.id).filter((id) => !kept.includes(id))];
}

/** One row to put in order. */
export interface ArrangeItem {
  id: string;
  name: string;
  badge?: ReactNode;
  sub: ReactNode;
  state: "up" | "down" | "off";
  label: string;
}

/** The servers as rows to put in order. */
export function ArrangeServers({ servers, order, onOrder }: { servers: ServerInfo[]; order: string[]; onOrder: (ids: string[]) => void }) {
  const { t } = useApp();
  const items = servers.map((s): ArrangeItem => {
    const waiting = !s.local && !s.online && !s.last_seen;
    return {
      id: s.id,
      name: s.name,
      badge: s.local && <span className="badge">{t("srv.panelBadge")}</span>,
      sub: s.version ? t("srv.version", { v: s.version }) : t("srv.notYet"),
      state: s.online ? "up" : "down",
      label: s.online ? t("srv.online") : waiting ? t("srv.waiting") : t("srv.offline"),
    };
  });
  return <ArrangeList items={items} order={order} onOrder={onOrder} hint={t("arr.hint")} />;
}

function moved(list: string[], from: number, to: number): string[] {
  const next = [...list];
  const [id] = next.splice(from, 1);
  next.splice(to, 0, id);
  return next;
}

/** The value of a motion token, in milliseconds. */
function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

interface Drag {
  id: string;
  pointer: number;
  /** Where the dragged row stood, where the first row stands, and from one row to the next. */
  start: number;
  first: number;
  pitch: number;
  ids: string[];
  before: string[];
}

type Part = "up" | "down";

/**
 * Short rows to put in order: dragged by their handle (mouse, pen or touch), or moved with
 * their buttons and Alt+Up / Alt+Down. Rows that make way slide into place.
 */
export function ArrangeList({ items, order, onOrder, hint }: { items: ArrangeItem[]; order: string[]; onOrder: (ids: string[]) => void; hint: string }) {
  const { t, num, low } = useApp();
  const [said, setSaid] = useState("");
  const [dragging, setDragging] = useState<string | null>(null);
  const list = arranged(order, items);
  const byId = new Map(items.map((s) => [s.id, s]));
  const rows = useRef(new Map<string, HTMLLIElement>());
  const tops = useRef(new Map<string, number>());
  const drag = useRef<Drag | null>(null);
  const focusAfter = useRef<{ id: string; part: Part } | null>(null);
  const still = low || matchMedia("(prefers-reduced-motion: reduce)").matches;

  const say = (id: string, at: number, of: number) =>
    setSaid(t("arr.moved", { name: byId.get(id)?.name ?? id, n: num(at + 1), total: num(of) }));

  // Rows that changed place slide from where they stood (not the one under the pointer).
  useLayoutEffect(() => {
    const before = tops.current;
    const after = new Map<string, number>();
    for (const [id, el] of rows.current) after.set(id, el.offsetTop);
    tops.current = after;
    if (!still) {
      const duration = parseFloat(token("--dur-mid")) || 0;
      const easing = token("--ease-out") || "ease-out";
      for (const [id, el] of rows.current) {
        const was = before.get(id);
        const now = after.get(id)!;
        if (id === drag.current?.id || was === undefined || was === now) continue;
        el.animate([{ transform: `translateY(${was - now}px)` }, { transform: "none" }], { duration, easing });
      }
    }
    // A moved row keeps the focus on its button (or the other one, at an end).
    const f = focusAfter.current;
    focusAfter.current = null;
    if (f) {
      const row = rows.current.get(f.id);
      const at = list.indexOf(f.id);
      const part = f.part === "up" && at === 0 ? "down" : f.part === "down" && at === list.length - 1 ? "up" : f.part;
      row?.querySelector<HTMLButtonElement>(`[data-part="${part}"]`)?.focus();
    }
  }, [list.join()]);

  // Escape while dragging puts the list back as it was.
  useEffect(() => {
    if (!dragging) return;
    const esc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") drop(true);
    };
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [dragging]);

  const move = (id: string, by: number, part: Part) => {
    const from = list.indexOf(id);
    const to = from + by;
    if (from < 0 || to < 0 || to >= list.length) return;
    focusAfter.current = { id, part };
    onOrder(moved(list, from, to));
    say(id, to, list.length);
  };

  const keys = (e: KeyboardEvent, id: string) => {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    const up = e.key === "ArrowUp";
    move(id, up ? -1 : 1, up ? "up" : "down");
  };

  const grab = (e: PointerEvent<HTMLSpanElement>, id: string) => {
    if (e.button !== 0 || drag.current) return;
    const el = rows.current.get(id);
    if (!el) return;
    e.preventDefault();
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // a pointer that is already gone: the drag ends on the next event
    }
    const first = rows.current.get(list[0])!.offsetTop;
    const second = list.length > 1 ? rows.current.get(list[1])!.offsetTop : first + el.offsetHeight;
    drag.current = { id, pointer: e.clientY, start: el.offsetTop, first, pitch: second - first || el.offsetHeight, ids: list, before: list };
    setDragging(id);
  };

  const follow = (e: PointerEvent) => {
    const d = drag.current;
    const el = d && rows.current.get(d.id);
    if (!d || !el) return;
    const top = d.start + e.clientY - d.pointer;
    const from = d.ids.indexOf(d.id);
    const to = Math.max(0, Math.min(d.ids.length - 1, Math.round((top - d.first) / d.pitch)));
    // The row stays under the pointer wherever the list puts it.
    el.style.transform = `translateY(${top - (d.first + to * d.pitch)}px)`;
    if (to !== from) {
      d.ids = moved(d.ids, from, to);
      onOrder(d.ids);
    }
  };

  const drop = (cancel: boolean) => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    setDragging(null);
    const el = rows.current.get(d.id);
    if (el) {
      const from = el.style.transform;
      el.style.transform = "";
      if (!still && from) el.animate([{ transform: from }, { transform: "none" }], { duration: parseFloat(token("--dur-mid")) || 0, easing: token("--ease-out") || "ease-out" });
    }
    if (cancel) {
      onOrder(d.before);
      return;
    }
    const at = d.ids.indexOf(d.id);
    if (at !== d.before.indexOf(d.id)) say(d.id, at, d.ids.length);
  };

  return (
    <>
      <p className="arr-hint" id="arr-hint">
        {hint}
      </p>
      <ol className="arr-list" aria-describedby="arr-hint">
        {list.map((id, i) => {
          const s = byId.get(id)!;
          return (
            <li
              key={id}
              className={`arr-row ${dragging === id ? "is-dragging" : ""} ${s.state === "up" ? "" : "is-off"}`}
              ref={(el) => {
                if (el) rows.current.set(id, el);
                else rows.current.delete(id);
              }}
              onKeyDown={(e) => keys(e, id)}
            >
              <span
                className="arr-grip"
                aria-hidden="true"
                title={t("arr.drag")}
                onPointerDown={(e) => grab(e, id)}
                onPointerMove={follow}
                onPointerUp={() => drop(false)}
                onPointerCancel={() => drop(true)}
              >
                <Icon name="grip" size={20} />
              </span>
              <span className="arr-n" aria-hidden="true">
                {num(i + 1)}
              </span>
              <div className="arr-id">
                <div className="arr-name">
                  <span>{s.name}</span>
                  {s.badge}
                </div>
                <div className="arr-sub">{s.sub}</div>
              </div>
              <StatePill state={s.state} label={s.label} />
              <span className="arr-moves">
                <button className="arr-move" type="button" data-part="up" disabled={i === 0} aria-label={t("arr.up", { name: s.name })} title={t("arr.upKey")} onClick={() => move(id, -1, "up")}>
                  <Icon name="up" size={18} />
                </button>
                <button
                  className="arr-move"
                  type="button"
                  data-part="down"
                  disabled={i === list.length - 1}
                  aria-label={t("arr.down", { name: s.name })}
                  title={t("arr.downKey")}
                  onClick={() => move(id, 1, "down")}
                >
                  <Icon name="down" size={18} />
                </button>
              </span>
            </li>
          );
        })}
      </ol>
      <p className="sr-only" role="status" aria-live="polite">
        {said}
      </p>
    </>
  );
}
