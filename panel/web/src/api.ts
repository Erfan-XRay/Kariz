// The panel's JSON API. Addresses are relative, so the app works under any secret path.

export interface SessionInfo {
  authenticated: boolean;
  has_password: boolean;
  csrf?: string;
}

export interface ServerInfo {
  id: string;
  name: string;
  local: boolean;
  version: string;
  arch: string;
}

export interface SessionRow {
  id: number;
  ip: string;
  agent: string;
  created: number;
  last_seen: number;
  current: boolean;
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
  servers: () => call<{ servers: ServerInfo[] }>("GET", "servers"),
  sessions: () => call<{ sessions: SessionRow[] }>("GET", "sessions"),
  revoke: (id: number) => call<object>("POST", "sessions/revoke", { id }),
  changePassword: (current: string | undefined, next: string) =>
    call<object>("POST", "password", { current, new: next }),
  newLink: () => call<{ token: string; valid_for: number }>("POST", "links"),
};
