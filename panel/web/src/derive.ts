// Turns what each server reports into what the panel shows: a tunnel is one config on the
// entry server and one on the exit server, with the same name.
import type { ProbeStatus, ServerInfo, TunnelInfo, TunnelMeta, TunnelStatus } from "./api";

export type TunnelState = "up" | "down" | "off";

/** The servers with each tunnel's label and place from the panel put on it. */
export function withMeta(servers: ServerInfo[], meta: Record<string, TunnelMeta> | undefined): ServerInfo[] {
  if (!meta) return servers;
  return servers.map((s) => ({ ...s, tunnels: s.tunnels.map((t) => (meta[t.name] ? { ...t, meta: meta[t.name] } : t)) }));
}

export interface Side {
  server: ServerInfo;
  tunnel: TunnelInfo;
}

export interface Tunnel {
  name: string;
  /** The name the admin gave it to be shown by; null: its own name. */
  label: string | null;
  /** What the panel calls it: the label, else its name. */
  title: string;
  /** Its place as the admin arranged the tunnels; null: after those, as the servers list them. */
  position: number | null;
  /** Seconds the link has been up this time; null while it is down. */
  uptime: number | null;
  entry?: Side;
  exit?: Side;
  /** Both sides are known: only then it is drawn on the map. */
  paired: boolean;
  state: TunnelState;
  transport: string;
  /** An auto tunnel: the transport its sessions use right now. */
  via: string | null;
  profile: string;
  /** The cipher setting (`auto` when an older agent does not say). */
  encryption: string;
  /// Mbit/s and connections, from the entry side's counters.
  rate: number;
  connections: number;
  rtt: number | null;
  error: string | null;
  /** The entry side's health probe (Kariz 2.4 and newer). */
  probe: ProbeStatus | null;
  /** With mux: streams in use and the most the connections take, on the fuller side. */
  capacity: { streams: number; max: number } | null;
  /** Connected, and still users cannot get through: every stream taken, the target down, or no way through. */
  blocked: "full" | "target" | "path" | null;
}

/** Failed probes in a row (15 s apart) before the panel shows a tunnel as letting no one through. */
const BLOCKED_AFTER = 2;

/** Streams in use over the most, on the side that is fuller. */
function capacityOf(statuses: (TunnelStatus | null | undefined)[]): { streams: number; max: number } | null {
  let worst: { streams: number; max: number } | null = null;
  for (const s of statuses) {
    const streams = s?.peer.streams;
    const max = s?.peer.streams_max;
    if (streams == null || !max) continue;
    if (!worst || streams / max > worst.streams / worst.max) worst = { streams, max };
  }
  return worst;
}

function blockedOf(probe: ProbeStatus | null, capacity: { streams: number; max: number } | null): Tunnel["blocked"] {
  if (!probe) return null;
  if (probe.outcome === "target_failed" && probe.target_fails >= BLOCKED_AFTER) return "target";
  if (probe.outcome !== "failed" || probe.fails < BLOCKED_AFTER) return null;
  const full = (probe.error ?? "").includes("is full") || (!!capacity && capacity.streams >= capacity.max);
  return full ? "full" : "path";
}

export function pairTunnels(servers: ServerInfo[]): Tunnel[] {
  const byName = new Map<string, { entry?: Side; exit?: Side; other: Side[] }>();
  for (const server of servers) {
    for (const tunnel of server.tunnels) {
      const slot = byName.get(tunnel.name) ?? { other: [] };
      const side = { server, tunnel };
      if (tunnel.role === "entry" && !slot.entry) slot.entry = side;
      else if (tunnel.role === "exit" && !slot.exit) slot.exit = side;
      else slot.other.push(side);
      byName.set(tunnel.name, slot);
    }
  }
  const list = [...byName.entries()].map(([name, { entry, exit }]): Tunnel => {
    const any = entry ?? exit!;
    const meta = entry?.tunnel.meta ?? exit?.tunnel.meta;
    const sides = [entry, exit].filter((s): s is Side => !!s);
    const connected = entry?.tunnel.status?.peer.connected ?? exit?.tunnel.status?.peer.connected ?? false;
    const stopped = sides.some((s) => s.tunnel.active === false);
    const state: TunnelState = connected ? "up" : stopped ? "off" : "down";
    const status = entry?.tunnel.status ?? null;
    const probe = status?.probe ?? null;
    const capacity = capacityOf([status, exit?.tunnel.status]);
    return {
      name,
      label: meta?.label || null,
      title: meta?.label || name,
      position: meta?.position ?? null,
      uptime: connected ? entry?.tunnel.status?.peer.connected_secs ?? exit?.tunnel.status?.peer.connected_secs ?? null : null,
      entry,
      exit,
      paired: !!entry && !!exit,
      state,
      transport: any.tunnel.transport,
      via: entry?.tunnel.status?.peer.transport ?? exit?.tunnel.status?.peer.transport ?? null,
      profile: any.tunnel.profile,
      encryption: any.tunnel.encryption || "auto",
      rate: state === "up" ? entry?.tunnel.rate_mbps ?? exit?.tunnel.rate_mbps ?? 0 : 0,
      connections: status ? status.totals.tcp_open + status.totals.udp_flows : 0,
      rtt: status?.peer.rtt_ms ?? exit?.tunnel.status?.peer.rtt_ms ?? null,
      error: sides.find((s) => s.tunnel.error)?.tunnel.error ?? status?.peer.last_error?.text ?? null,
      probe,
      capacity,
      blocked: state === "up" ? blockedOf(probe, capacity) : null,
    };
  });
  // The arranged ones first, in their order; the others after them, as the servers list them.
  const at = (x: Tunnel) => x.position ?? Number.MAX_SAFE_INTEGER;
  return list.map((x, i) => [x, i] as const).sort(([a, i], [b, j]) => at(a) - at(b) || i - j).map(([x]) => x);
}

/** "12.3 Mbit/s" style numbers: Mbps below a gigabit. */
export function rateParts(mbps: number): { value: number; unit: string; decimals: number } {
  if (mbps >= 1000) return { value: mbps / 1000, unit: "Gbps", decimals: 2 };
  return { value: mbps, unit: "Mbps", decimals: mbps < 100 ? 1 : 0 };
}

export function bytesPerSec(bps: number): { value: number; unit: string; decimals: number } {
  const bits = bps * 8;
  if (bits >= 1e9) return { value: bits / 1e9, unit: "Gbps", decimals: 2 };
  if (bits >= 1e6) return { value: bits / 1e6, unit: "Mbps", decimals: 1 };
  return { value: bits / 1e3, unit: "kbps", decimals: 0 };
}
