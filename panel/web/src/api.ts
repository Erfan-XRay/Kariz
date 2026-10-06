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
export type LinkTransport = "auto" | "tcpmux" | "kcp" | "wss" | "quic";

/** The panel's address on a private (GRE) network link to another server: what that server's agent can dial. */
export interface GreAddr {
  link: string;
  network: string;
  /** The other server (id). */
  server: string;
  addr: string;
  peer_addr: string;
}

export interface PanelAddresses {
  v4: string | null;
  v6: string | null;
  agent_port: number | null;
  gre?: GreAddr[];
  /** The panel server's public IPv4 address as private network links use it, if it is known. */
  gre_local?: string | null;
}

/** Where the panel dials a server it connects to (reverse): its agent listens on `port` and the next one. */
export interface ReverseTarget {
  host: string;
  port: number;
  /** The one link transport; none: all of them in turn. */
  transport?: string | null;
}

/** A GRE link the panel is to make to a server: its public IPv4 address, and the network that gives the addresses (none: the first, or a new one). */
export interface NewGreLink {
  ip: string;
  network?: string;
}

/** What *Add server* over a private network made: the server (waiting for its agent) and its code. */
export interface GreJoin {
  code: string;
  valid_for: number;
  id: string;
  name: string;
  /** The private network the link's addresses come from. */
  network: string;
  /** The panel's end of the GRE link (what the agent dials) and the new server's. */
  panel_addr: string;
  server_addr: string;
}

export interface LinkError {
  at: number;
  /** `link_ended`: the link dropped or went quiet. `wrong_key`: an agent came with this server's id and a key that does not match. */
  kind: "link_ended" | "wrong_key";
  detail: string;
}

/** An automatic restart of a tunnel (by name) or of the running tunnels of a server (by id). */
export interface ScheduleRow {
  kind: "tunnel" | "server";
  subject: string;
  mode: "every" | "daily";
  every_secs: number;
  /** For `daily`: minutes after midnight, UTC. */
  daily_min: number;
  enabled: boolean;
  since: number;
  last_run: number;
  /** `ok`, `ok:N`, `skipped_offline`, `skipped_stopped`, `skipped_none`, `busy`, `missed`, or `failed:` and why. */
  last_result: string;
  next_run: number | null;
}

export interface ScheduleBody {
  kind: "tunnel" | "server";
  subject: string;
  mode: "every" | "daily";
  every_secs?: number;
  daily_min?: number;
  enabled: boolean;
}

export interface ServerInfo {
  id: string;
  name: string;
  local: boolean;
  online: boolean;
  version: string;
  arch: string;
  hostname: string;
  /** The address other servers reach it at (private networks), if one was set. */
  addr: string | null;
  /** The address it is known by (public IPv4, else IPv6): what private networks use until one is set. */
  addr_default?: string | null;
  seen_secs: number | null;
  /** When the panel last heard from it (unix seconds), also from before the panel started. */
  last_seen?: number | null;
  /** Why its link last ended or was refused, until it connects again. */
  last_error?: LinkError | null;
  /** The private network link its agent connects through, when it does. */
  gre?: { link: string; network: string } | null;
  /** Where the panel dials it, for a server the panel connects to (reverse). */
  reverse?: ReverseTarget | null;
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
  /** wss with a real certificate: the name it was issued for, which the dialing side checks it against. */
  tls_host?: string;
  /** `auto` (left out), `aes-256-gcm`, `chacha20-poly1305` or `none`. */
  encryption?: string;
  /** quic: seal every UDP packet with a key from the token (`[tunnel.quic] obfs`). */
  quic_obfs?: boolean;
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
  quic_obfs?: boolean;
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

/** The Telegram bot's settings as the panel shows them: the token itself is never sent. */
export interface TelegramView {
  enabled: boolean;
  token_set: boolean;
  token_tail: string;
  chats: { id: number; name: string }[];
  lang: "fa" | "en";
  servers: boolean;
  tunnels: boolean;
  server_grace: number;
  tunnel_grace: number;
  digest_hours: number;
  api_base: string;
  proxy: string;
  connect_via: string;
  muted_until: number | null;
  pair: { code: string; expires_in: number } | null;
  last_ok: number | null;
  last_error: string | null;
}

export type TelegramChange = Partial<
  Pick<TelegramView, "enabled" | "lang" | "servers" | "tunnels" | "server_grace" | "tunnel_grace" | "digest_hours" | "api_base" | "proxy" | "connect_via">
> & { token?: string; remove_token?: boolean };

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
  panelAddresses: () => call<PanelAddresses>("GET", "servers/panel-addresses"),
  joinGre: (name: string | undefined, ip: string, network: string | undefined, transport: LinkTransport) =>
    call<GreJoin>("POST", "servers/join-gre", { name, ip, network, transport }),
  removeServer: (id: string) => call<object>("POST", "servers/remove", { id }),
  /** With `gre`, a GRE link to the server is made first and the link to the panel goes across it (the answer then says what was made). */
  reconnectServer: (id: string, host: string, transport: LinkTransport, reverse?: { host: string; port: number }, gre?: NewGreLink) =>
    call<{ code: string; valid_for: number } & Partial<GreJoin>>("POST", "servers/reconnect", { id, host, transport, reverse, gre }),
  /** With `gre`, the panel makes a GRE link to the new server and dials it across that link (`host` is not used). */
  joinReverse: (name: string | undefined, host: string, port: number, transport: LinkTransport, gre?: NewGreLink) =>
    call<{ code: string; valid_for: number; id: string; name: string } & Partial<GreJoin>>("POST", "servers/join-reverse", { name, host, port, transport, gre }),
  renameServer: (id: string, name: string) => call<object>("POST", "servers/rename", { id, name }),
  schedules: () => call<{ schedules: ScheduleRow[] }>("GET", "schedules"),
  setSchedule: (body: ScheduleBody) => call<object>("POST", "schedules", body),
  deleteSchedule: (kind: string, subject: string) => call<object>("POST", "schedules/delete", { kind, subject }),
  runSchedule: (kind: string, subject: string) => call<object>("POST", "schedules/run", { kind, subject }),
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
  speedtestStop: (name: string, id: string) => call<{ ok: boolean }>("POST", "tunnels/speedtest/stop", { name, id }),
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
  telegram: () => call<TelegramView>("GET", "telegram"),
  telegramSet: (body: TelegramChange) => call<TelegramView>("POST", "telegram", body),
  telegramPair: () => call<TelegramView>("POST", "telegram/pair", {}),
  telegramRemoveChat: (id: number) => call<TelegramView>("POST", "telegram/chats/remove", { id }),
  telegramTest: () => call<{ ok: boolean; bot?: string; error?: string }>("POST", "telegram/test", {}),
  applyUpdate: (confirm_major: boolean) => call<{ op: string }>("POST", "update/apply", { confirm_major }),
  updateServers: (restart_tunnels: boolean) => call<{ op: string }>("POST", "update/servers", { restart_tunnels }),
  newLink: () => call<{ token: string; valid_for: number }>("POST", "links"),
};
