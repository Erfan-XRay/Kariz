import { useEffect, useState } from "react";
import { api } from "./api";
import type { Op } from "./api";
import { useApp } from "./store";
import { Icon } from "./ui";

/** Follows an operation the panel runs in the background until it ends. */
export function useOp(id: string | null): Op | null {
  const [op, setOp] = useState<Op | null>(null);
  useEffect(() => {
    if (!id) {
      setOp(null);
      return;
    }
    let alive = true;
    let timer = 0;
    const poll = async () => {
      try {
        const next = await api.op(id);
        if (!alive) return;
        setOp(next);
        if (next.state === "running") timer = window.setTimeout(poll, 400);
      } catch {
        if (alive) timer = window.setTimeout(poll, 1000);
      }
    };
    void poll();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [id]);
  return op;
}

type T = (k: string, v?: Record<string, string | number>) => string;

/**
 * The text of an error: a known code (`gre_blocked`, or one with details after colons such as
 * `no_address:SERVER`, `overlaps_route:SERVER:10.0.0.0/8`), or what the server said.
 */
export function describeError(t: T, error: string, names?: Map<string, string>): string {
  if (!error) return "";
  const [code, a, ...rest] = error.split(":");
  const key = `op.err.${code}`;
  const known = t(key);
  if (known === key) return error;
  const who = a ? (names?.get(a) ?? a) : "";
  return t(key, { server: who, route: rest.join(":"), detail: [a, ...rest].filter(Boolean).join(":") });
}

/** The text of an operation's error: a known code, or what the server said. */
export function opError(t: T, error: string | null, names?: Map<string, string>): string {
  return describeError(t, error ?? "", names);
}

/** The steps of an operation as a checklist that fills in as they finish. */
export function Checklist({ op }: { op: Op | null }) {
  const { t } = useApp();
  if (!op) return null;
  return (
    <ul className="checklist" aria-live="polite">
      {op.steps.map((s, i) => (
        <li key={`${s.id}-${i}`} className={s.state === "fail" ? "fail" : s.state}>
          <span className="st">{s.state === "ok" && <Icon name="check" size={14} />}{s.state === "fail" && <Icon name="x" size={14} />}</span>
          <span>{t(`op.${s.id}`)}</span>
          <span />
        </li>
      ))}
    </ul>
  );
}
