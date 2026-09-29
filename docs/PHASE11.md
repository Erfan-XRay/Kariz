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

*Status after 11.2:* `auth.rs` (the rules, taking `now` so tests move time) and `api.rs`
(the JSON API) implement section 3. Passwords are Argon2id (19 MiB, 2 passes); login
links, sessions and the CSRF value are random 32-byte tokens stored as BLAKE3 hashes, so
a stolen database opens nothing. The cookie is `__Host-kariz` (`Secure`, `HttpOnly`,
`SameSite=Strict`, `Path=/`). Changes need `X-Kariz-CSRF`; sign-in requests need a JSON
body (a form post from another site is refused before it is read). Five failed tries from
one address (a wrong password, a wrong or spent link, a wrong current password) lock it
out for 15 minutes, right passwords included. A session ends after 7 idle days, at most 30
exist (the least recently used goes), and changing the password ends all the others.
Endpoints: `GET /api/session`, `POST /api/login`, `POST /api/link`, `POST /api/logout`,
`GET /api/sessions`, `POST /api/sessions/revoke`, `POST /api/password`, `POST /api/links`.
The audit log records sign-ins, failures, revocations and password changes. CLI:
`kariz-panel login-link [--host H]` and `kariz-panel reset-password [--stdin]` (it makes a
random password unless told to read one). Tests: 4 for the rules and 6 through the router
(cookie flags, CSRF, JSON-only, one-time links, the lockout, sessions and the password).
Checked by hand with the binary and `curl -k`: `init`, `login-link`, a link signing in
once, CSRF refused without the header, `reset-password` ending the old session, and five
wrong tries turning the right password into a 429.

*Status after 11.3:* the web app is the prototype's design as React 19 and TypeScript
(`panel/web/src`): the tokens, the three fonts and both themes as they were in phase 9;
Persian (RTL) and English, Persian or Latin digits, low power (also on with the browser's
reduced-motion setting); the preferences are kept in `localStorage`, and the app works
without it. What runs on the real API:

- **Boot and sign-in:** the logo draws itself while `GET /api/session` is asked; the login
  scene (stars, dunes, the wells, the channel) ripples red on a wrong try and descends
  into the dashboard on a right one. Password or a pasted link or code; `#t=...` in the
  address signs in by itself and is removed from the address bar at once. The messages
  say how many tries are left, or for how long the address is locked out.
- **The shell:** the side rail (a bottom bar on phones), the sky strip, `Ctrl+K` with the
  pages, the theme, the language, low power, a new login link and sign-out. A session that
  ends elsewhere (revoked, a new password) takes the panel back to the sign-in page.
- **Map, Servers:** the wells of the servers the panel knows (`GET /api/servers`: for now
  the one it runs on, named by its hostname), the four numbers with rolling digits, the
  tunnels list (empty until 11.4). Tunnels and Logs show that they come in phase 12.
- **Settings:** the admin password (set, change, with a strength meter), one-time login
  links with a countdown and a copy button, the sessions (device, address, last seen,
  revoke) and the appearance settings.

Built size: 88 KB of JavaScript and 10 KB of CSS gzipped, and 180 KB of fonts (already
compressed): about 280 KB against the 400 KB budget. Checked in a real browser (Edge over
the DevTools protocol, real time, since a headless browser's virtual time never finishes
the animations): the login page, the lockout message, sign-in by link and its descent, the
map, servers and settings pages, English with the Dawn theme, the palette, a phone width,
and the whole password path (set a password, sign out, sign in with it).

*Status after 11.4:* agents work end to end. What was built:

- **`kariz::link`** (the core): `Acceptor` and `Dialer` make an authenticated, encrypted
  mux session with the tunnels' own handshake and records (transport `tcpmux`), and
  which end dials is independent of which end opens streams. The tunnel code does not use
  it. Tests: streams in both directions, and a wrong token never connects.
- **The protocol** (`wire.rs`): four requests, `hello`, `enroll`, `health`, `tunnels`,
  each one mux stream (the request as JSON in the open bytes, one JSON answer). There is no
  request that names a command, a path or a file. The tunnel report never carries a
  token (a test checks the serialized report).
- **Identity** (`hub.rs`, `agent.rs`, `join.rs`): a join code is `kz1_` and the base64 of
  the panel's address, its link token and a one-time join secret (10 minutes, stored as
  a hash, spent in one statement). A new agent shows the secret; the panel makes it an id
  and a 32-byte key, sends them over the link (`enroll`), and the agent saves them
  (`agent.toml`, mode 600, written beside the file and renamed). A registered agent
  answers a random challenge with `blake3::keyed_hash(key, challenge)`, compared in
  constant time. A second use of a code, a forged key and a second `enroll` are all
  refused (tested over real sockets).
- **Collecting** (`collect.rs`): CPU share, memory, network rates (loopback excluded),
  uptime and load from `/proc` (parsers tested on sample files; nothing is reported
  where there is no `/proc`), and each `*.toml` in the Kariz directory with systemd's
  state (`is-active kariz@NAME`) and, when the daemon runs, its `status` from the control
  socket of phase 10. The panel does the same for its own server, in process.
- **The panel** polls every agent every 2 s and keeps the latest; rates come from two
  readings of the entry side's byte counters. API: `GET /api/servers` (health and
  tunnels of every server), `POST /api/servers/join-code`, `POST /api/servers/remove`
  (the server leaves the panel and its link closes; nothing is deleted on it). Config:
  `agent_listen` (set by `init` to a random port) and `kariz_dir`.
- **The web app:** *Servers* with health meters, the state of each server, *Add server*
  (a name, the panel's address as the new server sees it, the join code, a countdown, and a
  wait that turns to "connected" when the server appears) and *Remove*. The map draws the
  tunnels whose two sides are both known (an entry and an exit with the same name), in
  the state the entry's daemon reports (flowing, broken, stopped); the four numbers and
  the tunnel table come from the same data. One-sided tunnels are listed as such.
- **Checked by hand,** with two real processes and a real browser: *Add server* made a
  code, `kariz-panel agent --join CODE` connected, the server appeared, and after a
  restart the agent came back with the same id and no code; the map drew the tunnel
  between the panel's server and `istanbul-1`.
- **Bug found on the way:** the panel opened each request stream and never finished its
  own side, so the agent's dropping the stream after its answer reset it; the request now
  finishes its side at once.
- **Not in this step:** *the panel dials the agent* (an agent behind a firewall that only
  allows inbound connections). Agents dial the panel, which needs no open port on the
  new server; the reverse is left for phase 12. The manager script's `--agent` line is
  step 11.5, so the dialog shows `kariz-panel agent --join CODE` for now. Updates are
  polled every 2 s rather than pushed (server-sent events), which is enough for a handful
  of servers.

*Status after 11.5:* `kariz-manager` installs the panel and the agent (section 6 of this
document, as planned, with `--agent CODE` instead of a flag of the install line):
`panel install [--port N] [--host H]`, `panel link`, `panel password`, `panel status`,
`panel logs`, `panel uninstall`, `--agent CODE`, `agent status | logs | remove`, and a
*Web panel and agent* entry (`w`) in the menu. `install` puts `kariz-panel` in place when the
archive has it (or with `--panel-binary`), and both `install` and `update` write the units
`kariz-panel.service` and `kariz-agent.service` and restart the running ones. The release
archives carry `kariz-panel` (built with the web app, for all three architectures), and a
version with a dash is published as a **prerelease**, so GitHub's "latest release", which
`install` and `update` use, stays on the last stable version. `docs/panel.md` is the user's
guide. CI's manager job installs the panel on a real systemd host, signs in with the link it
prints, makes a join code through the API, connects this same machine with
`kariz-manager --agent`, and checks that the agent shows online with health from `/proc` and
that the machine's tunnels are listed; then updates and removes everything.

## 7. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **11.0** Plan (done) | This document. | |
| **11.1** Skeleton (done) | Workspace, `panel/` crate, config, SQLite schema, axum on TLS under the secret path, the embedded app (or placeholder), CLI; `panel/web` scaffold; CI builds and tests both; the release carries `kariz-panel`. | `kariz-panel serve` answers on HTTPS; CI green. |
| **11.2** Sign-in (done) | Section 3, API and tests. | Tests for every rule in the table. |
| **11.3** Web app (done) | Section 5. | Build under the budget; the prototype's screens in the real app, both languages and themes. |
| **11.4** Agents (done) | Section 4: `kariz::link`, join codes, the agent, health and tunnels, both directions, live Servers and map. | An in-process test joins an agent and reads its health; a second in CI over real sockets. |
| **11.5** Installer, release (done) | Section 6, `docs/panel.md`, CHANGELOG, `0.8.0-beta`. | Installed on a clean Linux VM in CI (manager job), release with the panel. |
