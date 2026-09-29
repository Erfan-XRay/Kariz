# Phase 11 plan: the panel's base

Target release: **v0.8.0-beta**. Scope from [PHASE9.md](PHASE9.md#3-the-panel-plan-phases-10-to-13):
the `kariz-panel` crate, sign-in, the installer, agents and servers, the app shell.
Tunnels made from the panel, live monitoring and the rest of the screens are phase 12.

## 1. What a user gets in 0.8.0-beta

1. `kariz-manager` installs the panel on a server (a menu entry, or `kariz-manager panel
   install`) and prints its address and a one-time login link.
2. The link opens the panel: the loader, then the dashboard (the login page, with the
   descent, when there is no valid link or session).
3. **Servers:** the panel's own server is there already. *Add server* gives a one-line
   command; run on another server, it installs Kariz and the agent, and the server
   appears live: CPU, memory, network, version, uptime, and the tunnels it already has
   (made by the manager script), read-only for now.
4. **The map** shows the servers as wells on the horizon, and the tunnels the agents
   report as channels, with the live traffic from `kariz status` (phase 10).
5. **Settings, security:** admin password, one-time login links, active sessions.

Not yet: creating or editing tunnels, logs, speed tests, updates, backups, TOTP, Let's
Encrypt (phase 12); Playwright tests and the security review (phase 13).

## 2. The crate

```
Cargo.toml            the workspace: kariz (the core, as today) and panel/
panel/
  Cargo.toml          kariz-panel, depends on kariz (path "..")
  src/
    main.rs           CLI: serve | agent | login-link | reset-password | join-code
    config.rs         /etc/kariz-panel/panel.toml
    db.rs             SQLite (bundled): schema, migrations
    auth.rs           passwords, links, sessions, rate limits
    http.rs           axum: the API and the embedded web app
    link.rs           the panel <-> agent link (section 4)
    agent/            what an agent does on its server
  web/                the web app (React), built into panel/web/dist
```

- **One binary, two roles.** `kariz-panel serve` is the panel; `kariz-panel agent` runs on
  every other server. Same file, so installing and updating stay one download.
- **The core stays lean.** The `kariz` binary gains nothing; the panel is a separate
  crate. It uses the `kariz` library for the config parser (`kariz check`), the control
  socket client (`status`), and the link's transport and handshake.
- **Dependencies:** axum and tower-http (HTTP, compression), rusqlite with bundled
  SQLite (a static binary, like today), argon2, rust-embed (the web app inside the
  binary), rcgen (the self-signed certificate; already used by `kariz` for QUIC).
- **The web app is built before the binary:** `npm ci && npm run build` in `panel/web`,
  then `cargo build`. Without `dist/` (a plain `cargo build` or `cargo test`), the binary
  embeds a one-page placeholder, so Rust work never needs Node.

## 3. Sign-in (11.2)

As planned in PHASE9, section 3:

| Piece | How |
|---|---|
| Password | Optional, Argon2id (m = 19 MiB, t = 2, p = 1, the OWASP baseline), set with `kariz-panel reset-password` or in Settings |
| One-time link | `kariz-panel login-link` (the installer runs it) prints `https://HOST:PORT/SECRET-PATH/#t=TOKEN`: 32 random bytes, stored as a SHA-256 hash, valid 60 minutes, spent on first use. In the fragment, so it never reaches a log, a proxy or the browser's history (the page removes it at once) |
| Session | 32 random bytes in a cookie `__Host-kariz` (`Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`), stored hashed in SQLite with the device and IP, idle 7 days, at most 30; listed and revocable in Settings; signing out or changing the password ends them |
| CSRF | Every request that changes something carries `X-Kariz-CSRF` (a per-session value the page reads from `/api/session`); SameSite=Strict is the second line |
| Rate limits | Per IP: 5 failed sign-ins, then 15 minutes locked out (the audit log records it); per link token, one try |
| Secret path | The panel answers only under `/k-XXXXXX/` (6 random hex bytes); anything else gets the same 404 as a plain nginx, so a scanner sees nothing |
| TLS | Always. A self-signed certificate made at install (rcgen, ECDSA P-256, 10 years); the installer prints its SHA-256 fingerprint so the first visit can be checked. Let's Encrypt in phase 12 |
| Port | Random free port above 20000 at install, changeable in `panel.toml` |

## 4. The link between panel and agents (11.4)

**Kariz carries it.** An agent reaches the panel over a Kariz `tcpmux` connection (or
`wss` behind a CDN): the same handshake, encryption and mux as tunnels, so the
management traffic resists DPI like the tunnels do and needs no second protocol. The
library gains a small public API for it (`kariz::link`: dial or accept one mux session
with a token), used by nothing else.

**Two keys.** The mux handshake uses the panel's *link token*, one per panel, in every
join code. Right after it, the agent proves who it is with its own *agent key*
(HMAC-SHA256 over a challenge from the panel). A leaked link token alone opens nothing;
removing a server deletes its key.

**Join codes.** *Add server* makes `kz1_...`: base64 of the panel's addresses, port,
transport, link token, certificate pin (for `wss`) and a one-time join secret (valid 10
minutes). The agent spends the join secret once to register (hostname, architecture,
version) and receives its agent key, kept in `/etc/kariz-panel/agent.toml` (root only).

**Directions.** By default the agent dials the panel (it works behind NAT and opens no
port on the server). *The panel dials the agent* is the other choice in the dialog: the
agent then listens on a port and the join code goes the other way (the panel's address
is not needed, the agent's is).

**Requests.** One mux stream per request, a JSON line each way: `hello`, `health` (CPU,
memory, network rates, disk, uptime, load), `tunnels` (the `/etc/kariz/*.toml` files, their
`kariz check --json` summary, the systemd state and, when running, `kariz status`),
`version`. Phase 12 adds the ones that change things (write a config, start, stop,
logs, speed test, update), still from a fixed list: there is no remote shell.

**The panel's own server** is a node too, served in-process without a link.

## 5. The web app (11.3)

React 19, TypeScript, Vite, as decided in PHASE9. What the prototype
(design/prototype/) drew becomes components:

- the tokens (design/tokens.css), the three fonts, Night and Dawn, Persian and English,
  low power;
- boot and the qanat loader, the login page and the descent, one-time links;
- the shell: the side rail (bottom bar on phones), the sky strip, `Ctrl+K`;
- the map (servers and the tunnels agents report), Servers with *Add server*, Settings
  (security part); the other rail items show their phase.

Data comes from a small JSON API under the secret path (`/api/...`) and one
server-sent-events stream (`/api/live`) that pushes health and status every second. The
build must stay under about 400 KB compressed.

## 6. Installer (11.5)

`scripts/kariz.sh` gains:

| Command | Does |
|---|---|
| `kariz-manager panel install` | downloads the release (it carries `kariz-panel` now), writes `panel.toml` (port, secret path, certificate), creates the database, installs `kariz-panel.service`, starts it, prints the address, the certificate fingerprint and a login link |
| `kariz-manager panel link` | a new one-time login link |
| `kariz-manager panel password` | set or change the admin password |
| `kariz-manager panel uninstall` | stops and removes it (the database stays unless `--yes`) |
| `bash <(curl ...) --agent kz1_...` | installs Kariz and the agent, registers with the panel, installs `kariz-agent.service` |

The menu gets a *Web panel* entry for the first three.

*Status after 11.1:* the workspace has `panel/` (lib and the `kariz-panel` binary with
`init` and `serve`). `init` writes `panel.toml` with a random port (20000-59999) and a
secret path `k-XXXXXXXX`, makes the SQLite database (migrations, `meta` and `audit` so
far) and the self-signed certificate, and prints the certificate's SHA-256. `serve`
answers over TLS (the core's acceptor, HTTP/1.1) only under `/<path>/`; every other
address gets the nginx 404 page, plain HTTP gets nothing, and the app's pages carry a
strict Content-Security-Policy. The web app (`panel/web`: Vite, React 19, TypeScript, 70
KB gzipped for the scaffold) is embedded from `panel/web/dist`, or a placeholder page
when it is not built. Checked by hand: `init`, `serve`, `curl -k` on every kind of path,
and headless Edge rendering the app, which calls `api/version`. CI builds the web app
first in the `check`, `cross` and release jobs, and builds `kariz-panel` with `kariz` for
all three architectures. Tests: config, database, certificate, `init`, the routes, and
two over real TLS with the core's pinned client (a wrong pin and plain HTTP get nothing).
One thing found: axum's `nest` does not pass `/<path>/` (trailing slash) to the inner
router, so that address, the panel's front page, has its own route.

## 7. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **11.0** Plan (done) | This document. | |
| **11.1** Skeleton (done) | Workspace, `panel/` crate, config, SQLite schema, axum on TLS under the secret path, the embedded app (or placeholder), CLI; `panel/web` scaffold; CI builds and tests both; the release carries `kariz-panel`. | `kariz-panel serve` answers on HTTPS; CI green. |
| **11.2** Sign-in | Section 3, API and tests. | Tests for every rule in the table. |
| **11.3** Web app | Section 5. | Build under the budget; the prototype's screens in the real app, both languages and themes. |
| **11.4** Agents | Section 4: `kariz::link`, join codes, the agent, health and tunnels, both directions, live Servers and map. | An in-process test joins an agent and reads its health; a second in CI over real sockets. |
| **11.5** Installer, release | Section 6, `docs/panel.md`, CHANGELOG, `0.8.0-beta`. | Installed on a clean Linux VM in CI (manager job), release with the panel. |
