# The web panel

A web panel for Kariz: see your servers and tunnels on one page, with their traffic live.
It is a **beta** (v0.8.0-beta): it shows and connects; making and editing tunnels from the
panel comes in the next phase. Tunnels are still made with
[kariz-manager](manager.md) on each server, and the panel reads them.

## Install

On the server that will run the panel, as root. The beta is a prerelease, so name its
version:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh) install --version v0.8.0-beta
kariz-manager panel install
```

`panel install` writes the settings, makes the database and a certificate, starts the
service `kariz-panel`, and prints:

```text
  address       https://203.0.113.5:28443/k-7f3a9c2e/
  certificate   SHA-256 9f2c41e0...
  agents        port 22230 (open it in the firewall for the servers you add)
  sign in       https://203.0.113.5:28443/k-7f3a9c2e/#t=...
```

- **The address** has a secret path: the panel answers only there. Any other address gets
  the plain "404 Not Found" page of an nginx, so a scanner finds nothing.
- **The certificate** is self-signed, so the browser warns on the first visit. Compare
  the fingerprint it shows with the one printed here, then continue.
- **The sign-in link** works once, for 60 minutes. Open it in the browser: it signs you in
  and disappears from the address bar. Make another any time with
  `kariz-manager panel link`.
- **The ports** are random (above 20000). `--port N` chooses the panel's. Open both the
  panel's port (for you) and the agents' port (for your other servers) in the firewall.

Set a password in *Settings* if you want to sign in without a link (10 characters or
more). Or from the server: `kariz-manager panel password` (makes a random one and shows
it; `--stdin` reads yours).

## Connect another server

In the panel: *Servers*, *Add server*. Give it a name if you like, and check the address
(the panel's address as the new server reaches it). The panel makes a join code and shows a
command. On the other server, as root:

```bash
kariz-manager --agent kz1_...
```

(If Kariz is not installed there yet, this installs it first; a beta needs
`--agent kz1_... --version v0.8.0-beta`.) The server appears in the panel within seconds.

- **The agent dials the panel**, so the new server opens no port. It needs to reach the
  panel's *agents* port.
- **A code works once**, for 10 minutes. It carries a token, so keep it as secret as a
  password until it is used.
- The agent runs as the service `kariz-agent`: `kariz-manager agent status | logs | remove`.
- **Remove a server** in the panel (*Servers*, *Remove*): it leaves the panel and its link
  closes. Nothing is deleted on it; its tunnels keep running.

## What you see

- **Map:** each server is a well on the horizon; each tunnel whose entry and exit servers
  are both connected is a channel of water between them. Flowing water is traffic (the
  particles follow the throughput); a dashed empty channel is a stopped tunnel; a red pulse
  is one that is broken. Hover for details.
- **Servers:** CPU, memory, network, uptime and the number of tunnels of each server. (On
  Linux; nothing is shown where there is no `/proc`.)
- **Tunnels** come from the `*.toml` files in `/etc/kariz` on each server, with systemd's
  state and the running daemon's `kariz status`. A tunnel is drawn when its entry and exit
  have **the same name**, which is how `kariz-manager` names the two sides.
- **Settings:** the password, one-time login links, and the sessions (each device, its
  address, when it was last used, and *Revoke*).

Persian and English, Night and Dawn, and a low-power mode (also on when your browser asks for
less motion) are in the top bar and in *Settings*. `Ctrl+K` opens commands.

## Security

- Everything is over TLS, under a secret path; plain HTTP gets nothing.
- The password is stored as an Argon2id hash. Login links, session cookies and the CSRF
  values are random 32-byte tokens stored as hashes only, so the database alone opens
  nothing. The cookie is `__Host-kariz` (`Secure`, `HttpOnly`, `SameSite=Strict`).
- Five failed tries from one address (wrong password, wrong or spent link) lock it out for
  15 minutes.
- Agents talk to the panel over Kariz's own link (the tunnels' handshake, encryption and
  mux). A token alone is not enough: each agent also proves a key of its own, made when it
  joined. **Agents answer four fixed requests** (who are you, keep this identity, health,
  tunnels). There is no remote shell, and tunnel tokens are never sent to the panel.
- The database (`/var/lib/kariz-panel`, root only) holds the agents' keys, because the
  panel needs them to check the proofs. Guard it like the tunnel configs.

## Commands

```bash
kariz-manager panel install [--port N] [--host H]   # settings, service, address and a link
kariz-manager panel link [--host H]                 # a new one-time login link
kariz-manager panel password [--stdin]              # a new admin password
kariz-manager panel status | logs                   # the service, its address / follow the log
kariz-manager panel uninstall [--yes]
kariz-manager --agent CODE                          # connect this server to a panel
kariz-manager agent status | logs | remove
```

The menu has the same under *w*. Without the manager: `kariz-panel init`, `serve`,
`login-link`, `reset-password`, `agent --join CODE` (see `kariz-panel --help`).

## Files

| Path | What |
|---|---|
| `/etc/kariz-panel/panel.toml` | the panel's settings: `listen`, `path`, `agent_listen`, `data_dir`, `kariz_dir` |
| `/etc/kariz-panel/agent.toml` | an agent's identity (id and key) and the panel's address |
| `/var/lib/kariz-panel/` | the database, the certificate and its key |
| `kariz-panel.service`, `kariz-agent.service` | the systemd units |

## Updating and removing

`kariz-manager update` installs the new binaries and restarts the tunnels, the panel and
the agent. `kariz-manager panel uninstall` removes the panel (and asks before deleting its
database); `kariz-manager agent remove` removes the agent.

## Troubleshooting

- **A server does not show up:** `kariz-manager agent logs` on it. "could not connect"
  usually means the panel's agents port is closed in a firewall between them; a join
  code that was already used or has expired is refused (make a new one).
- **The map shows no tunnel:** both servers must be connected and the tunnel must have the
  same name on both. A tunnel with one side connected is listed in the table as
  "one side only".
- **A tunnel shows as broken:** the panel reports what the entry side's daemon says
  (`kariz status`): not connected, or no daemon running. Start with `kariz-manager status NAME`
  on the server.
- **Locked out:** wait 15 minutes, or make a link on the server with
  `kariz-manager panel link`.
