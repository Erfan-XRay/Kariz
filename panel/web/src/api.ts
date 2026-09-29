// The panel's JSON API. Addresses are relative, so the app works under any secret path.

export interface SessionInfo {
  authenticated: boolean;
  has_password: boolean;
  csrf?: string;
}

export interface Health {
  cpu_pct: number | null;
  mem_total: number | null;
  mem_used: number | null;
  rx_bps: number | null;
  tx_bps: number | null;
  uptime_secs: number | null;
  load1: number | null;
}

export interface TunnelStatus {
  peer: { connected: boolean; sessions: number | null; rtt_ms: number | null; last_error: { secs_ago: number; text: string } | null };
  totals: { bytes_up: number; bytes_down: number; tcp_open: number; udp_flows: number };
}

export interface TunnelInfo {
  name: string;
  role: string;
  mode: string;
  transport: string;
  profile: string;
  listen: string | null;
  remote: string | null;
  forwards: { listen: string; target: string; protocol: string }[];
  active: boolean | null;
  status: TunnelStatus | null;
  error: string | null;
  rate_mbps: number | null;
}

export interface ServerInfo {
  id: string;
  name: string;
  local: boolean;
  online: boolean;
  version: string;
  arch: string;
  hostname: string;
  seen_secs: number | null;
  health: Health | null;
  tunnels: TunnelInfo[];
}

export interface SessionRow {
  id: number;
  ip: string;
  agent: string;
  created: number;
  last_seen: number;
  current: boolean;
}

export interface ForwardSpec {
  listen: string;
  target: string;
  protocol: string;
}

/** What the wizard sends: one tunnel, both sides (docs/PHASE12.md). */
export interface PairRequest {
  name: string;
  entry: string;
  exit: string;
  mode: string;
  transport: string;
  profile?: string;
  listen: string;
  dial: string;
  pool?: number;
  ws_path?: string;
  ws_host?: string;
  tls_sni?: string;
  forwards: ForwardSpec[];
  rotate?: boolean;
}

/** One side of a tunnel as an agent keeps it (never with its token). */
export interface Spec {
  name: string;
  role: string;
  mode: string;
  transport: string;
  profile?: string;
  listen?: string;
  remote?: string;
  pool?: number;
  ws_path?: string;
  ws_host?: string;
  tls_sni?: string;
  tls_pin?: string;
  forwards: ForwardSpec[];
}

export interface PortOwner {
  proto: string;
  addr: string;
  port: number;
  process: string | null;
  pid: number | null;
}

export interface CheckReply {
  ok: boolean;
  error?: string;
  warnings: string[];
  conflicts: PortOwner[];
}

export interface Step {
  id: string;
  state: "run" | "ok" | "fail";
  detail: string | null;
}

export interface Op {
  id: string;
  kind: "create" | "edit" | "control" | "delete";
  name: string;
  state: "running" | "done" | "failed";
  steps: Step[];
  error: string | null;
  undone: boolean | null;
}

export interface EventRow {
  id: number;
  at: number;
  kind: string;
  subject: string;
  detail: string;
}

export interface LogLine {
  server: string;
  role: string;
  text: string;
}

export class ApiError extends Error {
  status: number;
  code: string;
  body: Record<string, unknown>;
  constructor(status: number, body: Record<string, unknown>) {
    super(String(body.error ?? status));
    this.status = status;
    this.code = String(body.error ?? "error");
    this.body = body;
  }
}

let csrf = "";

export function setCsrf(value: string | undefined) {
  csrf = value ?? "";
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET" && csrf) headers["X-Kariz-CSRF"] = csrf;
  const response = await fetch(`./api/${path}`, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data: Record<string, unknown> = {};
  try {
    data = await response.json();
  } catch {
    // an empty or non-JSON answer
  }
  if (!response.ok) throw new ApiError(response.status, data);
  return data as T;
}

export const api = {
  session: () => call<SessionInfo>("GET", "session"),
  login: (password: string) => call<{ csrf: string }>("POST", "login", { password }),
  loginWithLink: (token: string) => call<{ csrf: string }>("POST", "link", { token }),
  logout: () => call<object>("POST", "logout"),
  servers: () => call<{ servers: ServerInfo[]; agents: boolean }>("GET", "servers"),
  joinCode: (name: string | undefined, host: string) => call<{ code: string; valid_for: number }>("POST", "servers/join-code", { name, host }),
  removeServer: (id: string) => call<object>("POST", "servers/remove", { id }),
  sessions: () => call<{ sessions: SessionRow[] }>("GET", "sessions"),
  revoke: (id: number) => call<object>("POST", "sessions/revoke", { id }),
  changePassword: (current: string | undefined, next: string) =>
    call<object>("POST", "password", { current, new: next }),
  tunnelCheck: (body: PairRequest) => call<{ entry: CheckReply; exit: CheckReply }>("POST", "tunnels/check", body),
  createTunnel: (body: PairRequest) => call<{ op: string }>("POST", "tunnels", body),
  editTunnel: (body: PairRequest) => call<{ op: string }>("POST", "tunnels/edit", body),
  controlTunnel: (name: string, action: "start" | "stop" | "restart") => call<{ op: string }>("POST", "tunnels/control", { name, action }),
  deleteTunnel: (name: string) => call<{ op: string }>("POST", "tunnels/delete", { name }),
  tunnelSpec: (server: string, name: string) =>
    call<Spec>("GET", `tunnel?server=${encodeURIComponent(server)}&name=${encodeURIComponent(name)}`),
  op: (id: string) => call<Op>("GET", `op?id=${encodeURIComponent(id)}`),
  ports: (server: string) => call<{ ports: PortOwner[] }>("GET", `ports?server=${encodeURIComponent(server)}`),
  history: (key: string, range: string) => call<{ points: [number, number][] }>("GET", `history?key=${encodeURIComponent(key)}&range=${range}`),
  events: (limit = 100) => call<{ events: EventRow[] }>("GET", `events?limit=${limit}`),
  logs: (name: string, lines = 200) => call<{ lines: LogLine[] }>("GET", `logs?name=${encodeURIComponent(name)}&lines=${lines}`),
  speedtest: (name: string, seconds: number, streams: number, udp: boolean) =>
    call<{ ok: boolean; error: string | null; text: string }>("POST", "tunnels/speedtest", { name, seconds, streams, udp }),
  backup: (passphrase: string) => call<{ data: string }>("POST", "backup", { passphrase }),
  restore: (passphrase: string, data: string, replace: boolean) => call<{ servers: number; restart: boolean }>("POST", "restore", { passphrase, data, replace }),
  newLink: () => call<{ token: string; valid_for: number }>("POST", "links"),
};
