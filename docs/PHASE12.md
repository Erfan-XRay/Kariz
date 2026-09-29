# Phase 12 plan: tunnels and live monitoring in the panel

Target release: **v0.9.0**. Scope: everything the manager script does for tunnels can be done
from the panel, for a pair of servers at once, and the panel shows what the servers and
tunnels are doing as it happens. Screens come from the prototype of phase 9
(`design/prototype/`: wizard, tunnel detail, logs, settings); this phase builds them for real.

Backward compatibility is not a goal before v1.0 (the repository is public but nobody depends
on it yet): the agent protocol, `panel.toml` and the database may change between betas.

## 1. What a user gets in 0.9.0

1. **Make a tunnel from the panel:** pick the entry and exit servers, the mode, the
   transport and the ports; the panel makes the token, writes both configs, starts the two
   sides in the right order and shows them connect. Nothing is left half made if a step fails.
2. **Manage it:** start, stop, restart, edit (both sides in step), rotate the token, delete.
   Tunnels made with `kariz-manager` show up already (phase 11) and can be edited the same way.
3. **Watch it:** live throughput and connection counts, history charts, the round trip and
   the state of each side, the logs of both sides in one place.
4. **Test it:** a speed test from the tunnel's page (the manager's, run by the entry agent).
5. **Keep it:** a backup of the panel (settings, servers, tunnel specs) to download, and a
   restore.

Not in this phase: private GRE networks (13), updating from the panel (14), the security
review, the browser test suite and the full documentation (15).

## 2. New agent requests

Still a **fixed list of data requests**, one mux stream each (PHASE11 section 4); none of them
takes a command or a path from the panel.

| Request | Does | Notes |
|---|---|---|
| `tunnel_check {spec}` | validates a tunnel spec, and reports port conflicts | uses `kariz::config`; the same checks as `kariz check`, plus who holds each port |
| `tunnel_put {spec}` | writes `/etc/kariz/NAME.toml` (0600) from a **structured spec** | the agent renders the TOML itself, so the panel cannot put arbitrary text in the file; refuses a name that is not `[a-z0-9-]{1,32}`; keeps the previous file as `.bak` |
| `tunnel_ctl {name, action}` | `start`, `stop`, `restart`, `enable`, `disable` | maps to `systemctl <action> kariz@NAME` with the name checked |
| `tunnel_delete {name}` | stops it, removes the config | |
| `tunnel_get {name}` | the spec, **without the token** (the token is replaced by whether one is set) | editing sends the token again only when it is changed |
| `ports {}` | listening TCP/UDP ports and the process that owns each | `/proc/net/*` and `/proc/*/fd` |
| `logs {name, lines, since}` | the last lines of the tunnel's journal | `journalctl -u kariz@NAME`, fixed arguments; polled, no stream held open |
| `speedtest {name, seconds, streams, udp}` | runs the entry side's speed test and returns the result | same code as `kariz-manager speedtest`; refused on an exit-side tunnel |

`hello` also lists the requests an agent understands, so a panel newer than an agent says
"update this server" instead of failing (used for real in phase 14).

**The token** is the one secret that travels: it is made by the panel, sent once to each
agent inside the encrypted link, and written 0600. The panel does **not keep it**: an edit sends
no token (each agent keeps its own file's), and rotating makes a new one for both sides. It
is never in an API answer.

*Status after 12.1:* the requests are in `panel/src/manage.rs` and the agent's handler; the
spec is `wire::Spec`. Every text field of a spec must be one plain line (no control
characters): a test found that a newline inside `listen` was accepted, escaped into a
multi-line string, and it is now refused. `tunnel_check` names the owner of each port a spec
wants, except the ports the tunnel of the same name already holds. The `hello` list of
understood requests moves to phase 14, where it is first needed.

## 3. Making a pair, safely

The panel does this as an ordered plan and stops at the first failure, undoing what it did:

1. `tunnel_check` on both servers (validity, ports free, name unused).
2. In direct mode, the **listening side first**; in reverse mode the entry (which listens).
3. `tunnel_put` on both, `tunnel_ctl start` on the listening side, then the dialing side.
4. Wait, up to 30 s, for `status` on both sides to say connected; show each step live.
5. On failure: stop and delete what was made on both, and say which step failed and why (the
   diagnosis of the prototype: refused, token mismatch, timeout, address not reachable).

Edit is the same plan with `.bak` restores as the undo. The list of steps is shown before it
starts, and each step's result after (a progress dialog, as designed).

*Status after 12.2:* `panel/src/pair.rs` makes, edits, controls and deletes a pair as an
operation that runs in the background (`POST /api/tunnels`, `/edit`, `/control`, `/delete`
answer `202` with an operation id; `GET /api/op?id=` returns its steps as they happen, and
`POST /api/tunnels/check` and `GET /api/ports?server=` serve the wizard). Because the panel
keeps no tokens, an edit that fails after a **new token** was sent cannot put the old token
back, and says so (`undone: false`); every other failure is undone. A listening `wss` side
gets a self-signed certificate made by its agent, and its pin is passed to the dialing side.
The servers run their tunnels through a `Services` trait: systemd by default, or child
processes (`manage::Processes`), which is what the CI test uses and what a host without
systemd could use later. `panel/tests/pair.rs` (Linux) makes a pair between the panel's own
server and a real agent with real daemons, sends traffic through it, edits, rotates the token,
stops, deletes, and checks that a tunnel that cannot connect leaves nothing on either side.

*Status after 12.3 to 12.6:* the wizard (`Wizard.tsx`) and the tunnels page (`Tunnels.tsx`)
follow an operation's steps by polling `GET /api/op`; charts (`Chart.tsx`), the Logs page
(`Logs.tsx`) and the speed test, backup and restore dialogs (`Extras.tsx`) use the endpoints
`/api/history`, `/api/events`, `/api/logs`, `/api/tunnels/speedtest`, `/api/backup` and
`/api/restore`. History is `panel/src/history.rs`, the backup `panel/src/backup.rs`. Released
as 0.9.0.

## 4. Live monitoring

- **Live:** the servers are asked every 2 seconds and the browser asks the panel every 2.5
  seconds; the map, the Servers page and the tunnel page all read that. (Server-sent events were
  planned and dropped: they need another dependency and gain a fraction of a second.)
- **History:** the hub keeps the last hour per server and per tunnel in memory (2 s samples)
  and writes 5-minute averages to SQLite, kept 30 days, for the charts (1 h, 24 h, 7 d, 30 d).
- **Events:** a tunnel going down or up, a server going offline or back, and every change
  made from the panel, in one list (the audit log grows into this).
- **Logs page:** pick a tunnel, both sides interleaved by time, filter by level, follow.

## 5. Settings and backup

- **Certificate:** use your own (`cert_file` and `key_file` in `panel.toml`, reloaded on
  SIGHUP). Automatic Let's Encrypt is left out: it needs port 80 or DNS access and an ACME
  client, which is a lot of surface for a panel that is usually reached by IP. Revisit if asked.
- **Backup:** *Download backup* gives one encrypted file (the passphrase is asked): the
  panel's servers with their keys, and the link token; not the sessions, and not tunnels (the
  panel keeps none: they live on the servers). *Restore* reads it on a fresh panel; after a
  restart of the panel the agents connect again (the link token is in the file).
- **Password reset and links** stay as they are.

## 6. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **12.0** Plan | This document. | |
| **12.1** Agent (done) | The requests of section 2 with their validation, the spec renderer, port ownership; unit tests for every parser and rule. | A test drives each request against a real agent and checks the file written. |
| **12.2** The pair (done) | The plan of section 3 in the hub, with undo; API for create, edit, control, delete. | CI: a pair is made through the API on one host, carries traffic, is edited and deleted; a failure at each step leaves nothing behind. |
| **12.3** Wizard and pages (done) | The wizard, tunnel detail (state of both sides, edit, token rotation), Tunnels page actions, both languages and themes. | A tunnel made from the browser works. |
| **12.4** Monitoring (done) | `/api/live`, history and charts, events, the Logs page. | Charts fill in the driven browser; history survives a panel restart. |
| **12.5** Speed test and backup (done) | The speed test from the tunnel page, backup and restore, own certificate. | CI: back up, wipe, restore, agents come back. |
| **12.6** Release (done) | Docs (`panel.md`), CHANGELOG, `0.9.0`. | CI green; release. |
