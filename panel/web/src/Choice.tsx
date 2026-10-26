import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { Icon, Seg } from "./ui";

/**
 * One choice out of a few, as a row of buttons with what the chosen one means under it: the
 * tunnel forms' quiet stand-in for a wall of tiles, where every option explained itself at once.
 */
export function Choice<T extends string>({
  label,
  value,
  options,
  onChange,
  hint,
  mono,
  risky,
  children,
}: {
  label: string;
  value: T;
  options: [T, string][];
  onChange: (v: T) => void;
  /** What the chosen option does. */
  hint?: ReactNode;
  /** Options that are names from the wire (tcp, kcp): set in the code font. */
  mono?: boolean;
  /** The chosen option is the unsafe one: drawn in the danger colour. */
  risky?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className={`field choice${mono ? " is-mono" : ""}${risky ? " is-risk" : ""}`}>
      <span className="label">{label}</span>
      <Seg value={value} options={options} onChange={onChange} label={label} />
      {hint && <span className="help">{hint}</span>}
      {children}
    </div>
  );
}

/**
 * Settings most tunnels never change, folded away under one line that says what is set in them
 * now. It opens by itself when `show` turns true: a value in it is wrong, or was set before.
 */
export function More({ title, note, show, children }: { title: string; note?: string; show?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(!!show);
  useEffect(() => {
    if (show) setOpen(true);
  }, [show]);
  return (
    <details className="more" open={open} onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary>
        <Icon name="chevron" size={16} />
        <span className="more-t">{title}</span>
        {note && <span className="more-note">{note}</span>}
      </summary>
      <div className="more-body">{children}</div>
    </details>
  );
}

/** The line under a folded group: what is set in it, or else what it holds. */
export function noteOf(set: (string | false | undefined)[], inside: (string | false | undefined)[]): string {
  const given = set.filter(Boolean);
  return (given.length ? given : inside.filter(Boolean)).join(" · ");
}

/** "label: value" for such a line, each part isolated so a Latin name in a Persian line keeps its order. */
export const pairOf = (label: string, value: string | number) => `⁨${label}⁩: ⁨${value}⁩`;
