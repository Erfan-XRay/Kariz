# The web panel

A web panel for Kariz: connect your servers, make and manage tunnels between them, and watch
their traffic live, from one page. **Everything is done here**; the [manager script](manager.md)
only installs Kariz and the panel. New to it? Start with [Using the panel](using-the-panel.md),
a walk through the screens in the order you need them.

## Install

On the server that will run the panel, as root:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh) install
kariz-manager panel install
```

`panel install` first asks how you will open the panel and gets a certificate for it (below),
then writes the settings, makes the database, starts the service `kariz-panel`, and prints:

```text
  address       https://203.0.113.5:28443/k-7f3a9c2e/
  certificate   /etc/letsencrypt/live/kariz-panel-203-0-113-5/fullchain.pem
  agents        port 22230: TCP and UDP, and TCP and UDP 22231 (open them for the servers you add)
  sign in       https://203.0.113.5:28443/k-7f3a9c2e/#t=...
```

- **The address** has a secret path: the panel answers only there. Any other address gets
  the plain "404 Not Found" page of an nginx, so a scanner finds nothing.
- **The certificate** is a real one from Let's Encrypt, so the browser shows no warning (and
  Safari keeps the sign-in cookie). See *The certificate* below.
- **The sign-in link** works once, for 60 minutes. Open it in the browser: it signs you in
  and disappears from the address bar. Make another any time with
  `kariz-manager panel link`.
- **The ports** are random (above 20000). `--port N` chooses the panel's. Open both the
  panel's port (for you) and the agents' port (for your other servers: TCP and UDP, and TCP and UDP on the next port
  for `wss` and `quic`) in the firewall.

The installer asks for an admin password of your own (at least 12 characters, three of: lower
case, UPPER CASE, digits, symbols); or choose the one-time sign-in link only. Change it in *Settings*
(10 characters or more), or from the server: `kariz-manager panel password` (asks the same way;
`--random` makes one and shows it, `--stdin` reads yours).

## The certificate

The panel is served only over a certificate the browser trusts, from Let's Encrypt. The installer
asks which of two you want:

- **A domain name** that points at this server (an A record). The certificate lasts 90 days.
- **The server's own public IP address**, if you have no domain. Let's Encrypt issues these for
  6 days at a time, so they are renewed more often. The address must be public (not `10.x`,
  `192.168.x` and so on).

```bash
kariz-manager panel install --domain panel.example.com      # or: --ip 203.0.113.5
kariz-manager panel cert --domain other.example.com         # change it later; --ip ADDRESS too
```

`panel cert` (also in the manager's menu) gets the new certificate, points the panel at it and
removes the old one; run it any time to change the domain, or to move from an IP address to a
domain. Add `--email you@example.com` to be told before a certificate expires.

- **Renewal is automatic:** a timer (`kariz-cert-renew.timer`) checks twice a day, and the panel
  loads the new certificate without a restart.
- **Port 80.** Let's Encrypt has to reach port 80 for a few seconds, at the first request and at
  each renewal. If a service holds it (nginx, a tunnel...), the manager says which one, asks to
  stop it for a moment and starts it again right after; the same happens at each renewal by
  itself. A process that is not a systemd service (a Docker container) cannot be stopped for you:
  free the port yourself. Open port 80 in the firewall.
- **certbot 5.4 or later** does this (IP address certificates need it). If the system has none
  that new, the manager installs one for itself in `/opt/kariz-certbot` with pip.
- **A certificate of your own:** `kariz-manager panel cert --cert-file F --key-file K` (Kariz does
  not renew it).

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
- **Choose the address.** *Add server* offers the panel's own public IPv4 and IPv6
  addresses. A server with only IPv6, or whose IPv4 route is filtered, can join over IPv6:
  the agents port listens on both.
- **The link's transport is automatic by default.** The panel takes agents over `tcpmux`
  (TCP on the agents port), `kcp` (UDP on the same port) and `wss` (WebSocket over TLS with
  the panel's certificate, on the next port; it looks like an ordinary HTTPS site) and
  `quic` (UDP on the next port, **always sealed**: every packet is encrypted with a key from
  the link token, so a network that filters QUIC does not recognise it, and the port answers
  nothing that was not made with the token; there is no setting for it, it is never plain
  QUIC). With *Auto*, an agent tries them in that order, moves on after two links in a row that carry no
  requests (it drops a link the panel has been quiet on for 20 s), and remembers the one
  that worked in `/etc/kariz-panel/link-transport`. Pick *TCP*, *KCP*, *WSS* or *QUIC* instead to
  make the code use only that one (the agent must be 1.4 or newer, and newer than 1.6 for
  *QUIC*). *Servers* shows which one each server uses.
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
   profile (`balanced`, `ultraspeed`, `gaming`). For `quic` there is a switch, on by default, to
   **seal every packet** (`obfs`, see [Transports](transports.md#obfs)); both servers need Kariz
   1.7 or newer for it.
3. **Connection:** the port the accepting side listens on and its address as the other side
   reaches it. For `ws`/`wss`, the path. In *direct* mode you can tick **Use a private GRE
   network** and the tunnel runs over private addresses the panel hands out
   ([networks.md](networks.md)).
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
  have **the same name**, which is how the panel names the two sides. Each tunnel's page
  has charts of its throughput and round trip over 1 hour, 24 hours, 7 days and 30 days. The
  last hour is kept in memory; five-minute averages are stored in the database for 30 days.
- **Networks:** private GRE networks between your servers, with addresses that are never
  repeated ([networks.md](networks.md)).
- **Logs:** pick a tunnel and read the log of **both sides interleaved by time**, filtered by
  level or text, with *Follow*. The *Events* tab lists a server going offline or back, a tunnel
  losing its connection or getting it back.
- **Speed test:** on a tunnel's page. The entry server measures download, upload and UDP
  through the tunnel ([Speed test](speedtest.md)); it uses the tunnel for the
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
kariz-manager panel password [--stdin | --random]  # a new admin password (asked twice, hidden)
kariz-manager panel status | logs                   # the service, its address / follow the log
kariz-manager panel uninstall [--yes]
kariz-manager --agent CODE                          # connect this server to a panel
kariz-manager agent status | logs | remove
```

The menu has the same (items 3 to 10). Without the manager: `kariz-panel init`, `serve`,
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

A file that cannot be read is refused and the old certificate stays. `kariz-manager panel cert`
does all of this for Let's Encrypt, with renewal (see *The certificate* above); with your own
tool (certbot, acme.sh) reload as above after each renewal.

## A host without systemd

In a container there is no systemd to start the tunnels. Set `services = "process"` in
`panel.toml` (or `agent.toml`): the panel or the agent then runs each tunnel as a child process
(`kariz run`), which stops when it stops. Tunnels then do not come back after a reboot by
themselves; that is the price of having no service manager.

## Files

| Path | What |
|---|---|
| `/etc/kariz-panel/panel.toml` | the panel's settings: `listen`, `path`, `agent_listen`, `data_dir`, `kariz_dir`, `services`, `cert_file`, `key_file`, `release_api`, `release_key` |
| `/etc/kariz-panel/agent.toml` | an agent's identity (id and key), the panel's address, `kariz_dir`, `services`, `release_key` |
| `/var/lib/kariz-panel/updates/` | the releases the panel downloaded, and the result of the last update |
| `/var/lib/kariz-panel/` | the database, the certificate and its key |
| `kariz-panel.service`, `kariz-agent.service` | the systemd units |

## Updating

When a newer release is out, a notice appears in the top bar (*Kariz 0.11.0 is out*), and
*Settings*, *Updates* shows it with the release notes. **The panel never updates by itself**:
it looks (once a day, or *Check now*), and updating is your button.

- **Update the panel:** the panel downloads the release for its own CPU **once**, checks its
  signature and checksum (see below), and hands over to a helper that runs as a service of its
  own, so that it lives on while the panel is replaced. The helper keeps the old programs as
  `kariz-panel.previous` and `kariz.previous`, puts the new ones in place with a rename (never
  half written), restarts the panel, and asks it over TLS (its certificate pinned) for its
  version. **If the new panel does not answer as the new version within 30 seconds, the old
  programs are put back and the panel restarted**; the result is kept, shown in *Updates*, and
  written to the audit log. The browser waits and reloads by itself. A new **major** version
  asks you to tick a box first, and a release older than the installed one is refused (no
  downgrades).
- **Update the other servers:** *Update the servers* (in the top bar when some are behind, and in
  *Updates*). The servers never need the internet: the panel sends the release it has, over the
  link that already exists, in pieces. Each agent checks the signature **again with its own copy
  of the key**, unpacks it, and hands over to its own helper, which swaps the programs, restarts
  the agent and waits for the new agent to reach the panel, putting the old programs back if it
  does not. The servers go **one at a time**, the panel waits for each to come back before the
  next, and a server that fails stops the rest, which are left as they were.
- **Their tunnels:** the tunnel daemons keep running the old program until they are restarted.
  Tick *Restart their tunnels afterwards* and they are restarted **one at a time**, each
  waited for until it is connected again, so a pair never loses both its sides at once. A tunnel
  drops for a moment; choose your time.
- **Channel:** *Stable* (the default) or *Beta*, which also offers prereleases.
- A server whose agent is too old to be updated this way (before 0.11) shows an error for that
  server; update it once by hand with `kariz-manager update`, and it can be updated from the
  panel from then on.

### The releases are signed

Every release archive has a checksum file and a signature (`.sha256` and `.sig`): an Ed25519
signature over the checksum file, which also names the archive, so an archive cannot be swapped
for another of the same release. The public key is built into `kariz-panel` and into
`kariz-manager`; the private key is a repository secret that only the release workflow sees. A
file whose signature does not verify is never run, and is deleted. The checksum alone would only
prove the download was not damaged: it comes from the same place as the file.

To check a download by hand: `kariz-panel release-verify kariz-v0.11.0-x86_64-linux.tar.gz`
(with its `.sha256` and `.sig` beside it). To pin a key of your own (a fork), set
`release_key = "HEX"` in `panel.toml` (and in `agent.toml` for agents), and `release_api =
"https://api.github.com/repos/OWNER/REPO"` to look somewhere else.

### If the panel cannot reach GitHub

The panel needs to reach `api.github.com` and GitHub's download addresses to find and fetch a
release. If it cannot, update by hand on each server with `kariz-manager update` (the manager
checks the signature too).

## Removing

`kariz-manager panel uninstall` removes the panel (and asks before deleting its database);
`kariz-manager agent remove` removes the agent.

## Troubleshooting

- **A server does not show up:** `kariz-manager agent logs` on it. "could not connect"
  usually means the panel's agents ports (TCP and UDP, and TCP and UDP on the next port) are closed
  in a firewall between them; "connected" repeating every 20 s or so means the path stalls
  TCP: with *Auto* the agent moves to `kcp`, then `wss`, then `quic`, by itself; a join
  code that was already used or has expired is refused (make a new one).
- **The map shows no tunnel:** both servers must be connected and the tunnel must have the
  same name on both. A tunnel with one side connected is listed in the table as
  "one side only".
- **A tunnel shows as broken:** the panel reports what the entry side's daemon says
  (`kariz status`): not connected, or no daemon running. Run `kariz status -c /etc/kariz/NAME.toml`
  on the server for the details ([Status](status.md)).
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
