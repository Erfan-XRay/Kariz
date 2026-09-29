# The web panel

A web panel for Kariz: connect your servers, make and manage tunnels between them, and watch
their traffic live, from one page. Tunnels made with [kariz-manager](manager.md) on a server
show up in the panel too, and the other way round: the panel and the manager share the same
files (`/etc/kariz/NAME.toml`) and services (`kariz@NAME`).

## Install

On the server that will run the panel, as root:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh) install
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

(If Kariz is not installed there yet, this installs it first.) The server appears in the
panel within seconds.

- **The agent dials the panel**, so the new server opens no port. It needs to reach the
  panel's *agents* port.
- **A code works once**, for 10 minutes. It carries a token, so keep it as secret as a
  password until it is used.
- The agent runs as the service `kariz-agent`: `kariz-manager agent status | logs | remove`.
- **Remove a server** in the panel (*Servers*, *Remove*): it leaves the panel and its link
  closes. Nothing is deleted on it; its tunnels keep running.

## Make a tunnel

*Tunnels*, *New tunnel* opens the wizard (it needs two connected servers):

1. **Servers:** a name (small letters, digits, dashes) and which server is the *entry* (where
   your users connect) and which the *exit* (which reaches the real services).
2. **Kind:** *reverse* (the exit dials the entry, so the exit opens no port) or *direct* (the
   entry dials the exit), the transport (`tcp`, `tcpmux`, `ws`, `wss`, `quic`, `kcp`) and the
   profile (`balanced`, `ultraspeed`, `gaming`).
3. **Connection:** the port the accepting side listens on and its address as the other side
   reaches it. For `ws`/`wss`, the path.
4. **Ports:** what the entry opens: `443, 8080-8090, 2053=53`, with TCP, UDP or both, and the
   host the exit reaches them at (default `127.0.0.1`).
5. **Build:** the review, then the panel does it on both servers, in order, and you watch each
   step: both servers check the settings and that the ports are free (a port that something
   else holds is named, with the program), the accepting side is written and started first,
   then the dialing side, and the panel waits (up to 30 seconds) until both report *connected*.

**If any step fails, everything made so far is removed from both servers**, and the wizard says
which step and why (a refused connection, a wrong token, a port in use...). A `wss` accepting
side gets a self-signed certificate made by its own server, and the other side is given its pin
automatically.

Open a tunnel (click its row) to see both sides, its charts, and to **edit** it (the same
wizard, filled in; both sides change together and are restarted, and if they do not connect
again the old settings are put back), **start / stop / restart** it, give it **a new token**, run
a **speed test**, or **delete** it (both sides).

The panel never keeps a tunnel's token: it makes one, sends it to the two servers over the link
and forgets it. So *a new token* cannot be undone if it fails half way; run it again.

## What you see

- **Map:** each server is a well on the horizon; each tunnel whose entry and exit servers
  are both connected is a channel of water between them. Flowing water is traffic (the
  particles follow the throughput); a dashed empty channel is a stopped tunnel; a red pulse
  is one that is broken. Hover for details.
- **Servers:** CPU, memory, network, uptime and the number of tunnels of each server. (On
  Linux; nothing is shown where there is no `/proc`.)
- **Tunnels** come from the `*.toml` files in `/etc/kariz` on each server, with systemd's
  state and the running daemon's `kariz status`. A tunnel is drawn when its entry and exit
  have **the same name**, which is how `kariz-manager` names the two sides. Each tunnel's page
  has charts of its throughput and round trip over 1 hour, 24 hours, 7 days and 30 days. The
  last hour is kept in memory; five-minute averages are stored in the database for 30 days.
- **Logs:** pick a tunnel and read the log of **both sides interleaved by time**, filtered by
  level or text, with *Follow*. The *Events* tab lists a server going offline or back, a tunnel
  losing its connection or getting it back.
- **Speed test:** on a tunnel's page. The entry server measures download, upload and UDP
  through the tunnel (the same as `kariz-manager speedtest`); it uses the tunnel for the
  seconds you choose.
- **Settings:** the password, one-time login links, the sessions (each device, its address,
  when it was last used, and *Revoke*), the backup, and the appearance.

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
  joined. **Agents answer a fixed list of requests**: who are you, keep this identity, health,
  tunnels, check / write / read / delete a tunnel, start / stop / restart it, listening ports,
  the log, the speed test. **There is no remote shell:** a request carries data (a tunnel's
  settings as typed fields, which the agent turns into the file itself, refusing anything with a
  control character in it), never a command or a path, and the programs an agent runs
  (`systemctl`, `journalctl`, `kariz speedtest`) get fixed arguments and a checked name.
- A tunnel's token is made by the panel, sent once to its two servers inside the encrypted
  link, and written there with mode 0600. The panel does not keep it and never shows it.
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

## Backup and restore

*Settings*, *Download a backup* gives one file, locked with a passphrase you choose (10
characters or more; Argon2id and ChaCha20-Poly1305). It holds the servers this panel knows,
with the keys they prove themselves with, and the token they dial with. It does **not** hold
tunnels (they live on the servers, in their own files, and the panel reads them from there),
sessions or login links. Keep the passphrase: without it the file cannot be opened.

To move to a new server: install the panel there, sign in, *Restore a backup*, then
`systemctl restart kariz-panel` so the agents can connect with the restored token. They dial
the panel's address, so point that name at the new server (or run `kariz-panel agent` again on
each server with a new join code if the address changed).

## Your own certificate

Set both in `panel.toml` and restart, or reload without dropping anything:

```toml
cert_file = "/etc/ssl/panel.pem"     # the certificate chain, PEM
key_file  = "/etc/ssl/panel.key"
```

```bash
systemctl kill -s HUP kariz-panel    # loads the new files for the next connections
```

A file that cannot be read is refused and the old certificate stays. Automatic Let's Encrypt
is not built in; use your own tool (certbot, acme.sh) and the reload above after each renewal.

## A host without systemd

In a container there is no systemd to start the tunnels. Set `services = "process"` in
`panel.toml` (or `agent.toml`): the panel or the agent then runs each tunnel as a child process
(`kariz run`), which stops when it stops. Tunnels then do not come back after a reboot by
themselves; that is the price of having no service manager.

## Files

| Path | What |
|---|---|
| `/etc/kariz-panel/panel.toml` | the panel's settings: `listen`, `path`, `agent_listen`, `data_dir`, `kariz_dir`, `services`, `cert_file`, `key_file` |
| `/etc/kariz-panel/agent.toml` | an agent's identity (id and key), the panel's address, `kariz_dir`, `services` |
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
- **The wizard says a port is in use:** it names the program (`sshd`, `nginx`, another
  tunnel). Pick another port, or stop that program. The check is done on the server itself, so
  a firewall rule does not matter here, but do open the port for your users.
- **A new tunnel did not connect:** the wizard shows the reason each side reported (for
  example *connection refused*: the accepting server's port is not reachable from the other, a
  firewall in between; or a timeout). Nothing is left behind; fix the cause and try again.
- **Logs are empty:** they come from `journalctl -u kariz@NAME`, so they need systemd; with
  `services = "process"` the daemons' output is not kept.
- **Locked out:** wait 15 minutes, or make a link on the server with
  `kariz-manager panel link`.
