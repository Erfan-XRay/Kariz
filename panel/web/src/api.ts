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
  /** IPv4 networks the server already routes. */
  routes?: string[];
}

export interface TunnelStatus {
  peer: { connected: boolean; transport?: string | null; sessions: number | null; rtt_ms: number | null; last_error: { secs_ago: number; text: string } | null };
  totals: { bytes_up: number; bytes_down: number; tcp_open: number; udp_flows: number };
}

export interface TunnelInfo {
  name: string;
  role: string;
  mode: string;
  transport: string;
  profile: string;
  /** The tunnel's cipher setting; empty from an older agent. */
  encryption?: string;
  listen: string | null;
  remote: string | null;
  forwards: { listen: string; target: string; protocol: string }[];
  active: boolean | null;
  status: TunnelStatus | null;
  error: string | null;
  rate_mbps: number | null;
}

export interface Latency {
  sent: number;
  received: number;
  p50_ms: number;
  p99_ms: number;
  jitter_ms: number;
}

export interface Rate {
  mbps: number;
  peak_mbps: number;
  bytes: number;
}

/** What a speed test measured (`kariz::speedtest::Report`). */
export interface SpeedReport {
  seconds: number;
  streams: number;
  idle: Latency;
  download: Rate;
  download_latency: Latency;
  upload: Rate;
  upload_latency: Latency;
  udp: Latency | null;
  notes: string[];
}

/** A speed test's answer; an older agent sends only `text`. */
export interface SpeedResult {
  ok: boolean;
  error: string | null;
  text: string;
  report?: SpeedReport | null;
}

/** A speed test that runs in the background (`start`), and what it has said since (`poll`). */
export interface SpeedStart {
  ok: boolean;
  error: string | null;
  id: string;
}
export interface SpeedPoll {
  ok: boolean;
  error: string | null;
  /** The progress lines of `kariz::speedtest::run`, after the ones already seen. */
  lines: string[];
  next: number;
  done: boolean;
  report: SpeedReport | null;
}

/** How an agent reaches the panel: `auto` tries every transport in turn. */
export type LinkTransport = "auto" | "tcpmux" | "kcp" | "wss";

export interface ServerInfo {
  id: string;
  name: string;
  local: boolean;
  online: boolean;
  version: string;
  arch: string;
  hostname: string;
  /** The address other servers reach it at (private networks), if set. */
  addr: string | null;
  seen_secs: number | null;
  /** The transport its agent's link uses now (tcpmux or kcp). */
  link?: string | null;
  health: Health | null;
  /** Its public IPv4 and IPv6 addresses, when known. */
  ip4?: string | null;
  ip6?: string | null;
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

/** What the wizard sends: one tunnel, both sides. */
/** A tunnel's mux settings; what is left out keeps the profile's value. */
export interface MuxSpec {
  /** Only ever false: mux off for ws, wss or kcp. */
  enabled?: boolean;
  connections?: number;
  max_streams?: number;
  /** Bytes. */
  stream_window?: number;
  max_lifetime_secs?: number;
  ping_interval_secs?: number;
  coalesce?: boolean;
}

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
  mux?: MuxSpec;
  /** wss: the files of a real certificate on the listening side. */
  tls_cert?: string;
  tls_key?: string;
  /** `auto` (left out), `aes-256-gcm`, `chacha20-poly1305` or `none`. */
  encryption?: string;
  forwards: ForwardSpec[];
  rotate?: boolean;
  /** Direct mode: run over this private GRE network (its id). */
  network?: string;
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
  mux?: MuxSpec;
  tls_cert?: string;
  tls_key?: string;
  encryption?: string;
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
  /** Servers that could not be reached and were left out. */
  offline?: string[];
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

export interface Network {
  id: string;
  name: string;
  cidr: string;
  created: number;
  links: number;
  capacity: number;
}

export interface Link {
  id: string;
  network: string;
  a: string;
  b: string;
  subnet: string;
  addr_a: string;
  addr_b: string;
  gre_key: number;
  ifname: string;
}

export interface UpdateStatus {
  current: string;
  configured: boolean;
  channel: "stable" | "beta";
  auto: boolean;
  state: "none" | "newer" | "same" | "older";
  major: boolean;
  latest: { tag: string; version: string; notes: string; prerelease: boolean } | null;
  checked_at: number | null;
  error: string | null;
  last_result: { version: string; ok: boolean; rolled_back: boolean; error: string | null; at: number } | null;
  busy: boolean;
  /** Servers whose agent is older than this panel. */
  outdated: { id: string; name: string; version: string; online: boolean }[];
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
  joinCode: (name: string | undefined, host: string, transport: LinkTransport) =>
    call<{ code: string; valid_for: number }>("POST", "servers/join-code", { name, host, transport }),
  panelAddresses: () => call<{ v4: string | null; v6: string | null; agent_port: number | null }>("GET", "servers/panel-addresses"),
  removeServer: (id: string) => call<object>("POST", "servers/remove", { id }),
  sessions: () => call<{ sessions: SessionRow[] }>("GET", "sessions"),
  revoke: (id: number) => call<object>("POST", "sessions/revoke", { id }),
  changePassword: (current: string | undefined, next: string) =>
    call<object>("POST", "password", { current, new: next }),
  tunnelCert: (server: string, host: string, email?: string) =>
    call<{ ok: boolean; error: string | null; cert: string | null; key: string | null }>("POST", "tunnels/cert", { server, host, email }),
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
  speedtestStart: (name: string, seconds: number, streams: number, udp: boolean) =>
    call<SpeedStart>("POST", "tunnels/speedtest/start", { name, seconds, streams, udp }),
  speedtestPoll: (name: string, id: string, after: number) => call<SpeedPoll>("POST", "tunnels/speedtest/poll", { name, id, after }),
  speedtest: (name: string, seconds: number, streams: number, udp: boolean) =>
    call<SpeedResult>("POST", "tunnels/speedtest", { name, seconds, streams, udp }),
  backup: (passphrase: string) => call<{ data: string }>("POST", "backup", { passphrase }),
  restore: (passphrase: string, data: string, replace: boolean) => call<{ servers: number; restart: boolean }>("POST", "restore", { passphrase, data, replace }),
  networks: () => call<{ networks: Network[]; links: Link[] }>("GET", "networks"),
  createNetwork: (name: string, cidr: string) => call<Network>("POST", "networks", { name, cidr }),
  deleteNetwork: (id: string) => call<object>("POST", "networks/delete", { id }),
  createLinks: (network: string, servers: string[], hub?: string) => call<{ op: string }>("POST", "networks/links", { network, servers, hub }),
  deleteLink: (id: string) => call<object>("POST", "networks/links/delete", { id }),
  setAddress: (id: string, addr: string) => call<object>("POST", "servers/address", { id, addr }),
  update: () => call<UpdateStatus>("GET", "update"),
  checkUpdate: () => call<UpdateStatus>("POST", "update/check", {}),
  updateSettings: (body: { channel?: "stable" | "beta"; auto?: boolean }) => call<UpdateStatus>("POST", "update/settings", body),
  applyUpdate: (confirm_major: boolean) => call<{ op: string }>("POST", "update/apply", { confirm_major }),
  updateServers: (restart_tunnels: boolean) => call<{ op: string }>("POST", "update/servers", { restart_tunnels }),
  newLink: () => call<{ token: string; valid_for: number }>("POST", "links"),
};
