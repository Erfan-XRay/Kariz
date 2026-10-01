import type { MuxSpec } from "./api";

/** The transports a person chooses from. `tcp` with mux on is the core's `tcpmux`; with mux off, plain `tcp`. */
export const TRANSPORTS = ["auto", "tcp", "ws", "wss", "quic", "kcp"] as const;

/** Transports where mux can be turned off (`auto` and `quic` always multiplex). */
export const MUX_OPTIONAL = ["tcp", "ws", "wss", "kcp"];

/** A tunnel's core transport (and mux settings) as the form shows them: a transport and whether mux is on. */
export function splitTransport(core: string, mux?: MuxSpec | null): { transport: string; mux: boolean } {
  if (core === "tcpmux") return { transport: "tcp", mux: true };
  if (core === "tcp") return { transport: "tcp", mux: false };
  return { transport: core, mux: mux?.enabled ?? true };
}

/** The form's transport and mux switch as the core's transport, and the `enabled` to write (left out where the default is right). */
export function joinTransport(transport: string, mux: boolean): { core: string; enabled?: boolean } {
  if (transport === "tcp") return { core: mux ? "tcpmux" : "tcp" };
  if (!mux && MUX_OPTIONAL.includes(transport)) return { core: transport, enabled: false };
  return { core: transport };
}

/** The transport as a list shows it: `tcpmux` is just tcp, and an auto tunnel says what it uses now. */
export function transportLabel(core: string, via?: string | null): string {
  if (core === "auto") return via ? `auto → ${via.replace(/tcpmux/g, "tcp")}` : "auto";
  return core === "tcpmux" ? "tcp" : core === "tcp" ? "tcp · no mux" : core;
}
