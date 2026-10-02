import type { ServerInfo } from "./api";

/** The port part of an address like `0.0.0.0:3080`. */
export const portOf = (addr: string | undefined) => addr?.split(":").pop() ?? "";
export const hostOf = (addr: string | undefined) => (addr ? addr.slice(0, addr.lastIndexOf(":")) : "");

/** The port a new tunnel starts from. */
export const FIRST_TUNNEL_PORT = 3080;

/** The ports on a server that its tunnels hold: the one a tunnel listens on (and the next one for an auto tunnel, which also listens for a WebSocket) and the forwarded ones. */
export function portsInUse(server: Pick<ServerInfo, "tunnels"> | undefined): Set<number> {
  const used = new Set<number>();
  for (const tunnel of server?.tunnels ?? []) {
    const own = +portOf(tunnel.listen ?? "");
    if (own) {
      used.add(own);
      if (tunnel.transport === "auto") used.add(own + 1);
    }
    for (const f of tunnel.forwards ?? []) {
      const fwd = +portOf(f.listen);
      if (fwd) used.add(fwd);
    }
  }
  return used;
}

/** The first port from 3080 up that none of the server's tunnels holds, so the second tunnel gets 3081, the third 3082, and none of them meets an error for a port that is taken. */
export function freeTunnelPort(used: Set<number>, transport: string): number {
  const free = (p: number) => !used.has(p) && (transport !== "auto" || !used.has(p + 1));
  let port = FIRST_TUNNEL_PORT;
  while (port < 65534 && !free(port)) port++;
  return port;
}
