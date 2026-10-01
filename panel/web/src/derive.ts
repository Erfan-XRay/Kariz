// Turns what each server reports into what the panel shows: a tunnel is one config on the
// entry server and one on the exit server, with the same name.
import type { ServerInfo, TunnelInfo } from "./api";

export type TunnelState = "up" | "down" | "off";

export interface Side {
  server: ServerInfo;
  tunnel: TunnelInfo;
}

export interface Tunnel {
  name: string;
  entry?: Side;
  exit?: Side;
  /** Both sides are known: only then it is drawn on the map. */
  paired: boolean;
  state: TunnelState;
  transport: string;
  /** An auto tunnel: the transport its sessions use right now. */
  via: string | null;
  profile: string;
  /// Mbit/s and connections, from the entry side's counters.
  rate: number;
  connections: number;
  rtt: number | null;
  error: string | null;
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
  return [...byName.entries()].map(([name, { entry, exit }]) => {
    const any = entry ?? exit!;
    const sides = [entry, exit].filter((s): s is Side => !!s);
    const connected = entry?.tunnel.status?.peer.connected ?? exit?.tunnel.status?.peer.connected ?? false;
    const stopped = sides.some((s) => s.tunnel.active === false);
    const state: TunnelState = connected ? "up" : stopped ? "off" : "down";
    const status = entry?.tunnel.status ?? null;
    return {
      name,
      entry,
      exit,
      paired: !!entry && !!exit,
      state,
      transport: any.tunnel.transport,
      via: entry?.tunnel.status?.peer.transport ?? exit?.tunnel.status?.peer.transport ?? null,
      profile: any.tunnel.profile,
      rate: state === "up" ? entry?.tunnel.rate_mbps ?? exit?.tunnel.rate_mbps ?? 0 : 0,
      connections: status ? status.totals.tcp_open + status.totals.udp_flows : 0,
      rtt: status?.peer.rtt_ms ?? exit?.tunnel.status?.peer.rtt_ms ?? null,
      error: sides.find((s) => s.tunnel.error)?.tunnel.error ?? status?.peer.last_error?.text ?? null,
    };
  });
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
