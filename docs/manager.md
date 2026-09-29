# The manager script

`scripts/kariz.sh` installs Kariz and sets up and runs its tunnels on a Linux server with
systemd. After the first run it is also the `kariz-manager` command.

## Install

As root, on each server:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
```

It opens a menu. Choose **1** to install: it finds your CPU (x86_64, aarch64 or armv7),
downloads the latest release, checks its SHA-256, and installs `kariz` in
`/usr/local/bin`, the systemd template `kariz@.service`, and the `kariz-manager` command.

While the repository is private, the download needs a token that can read it:

```bash
export GITHUB_TOKEN=ghp_...        # or run as: sudo GITHUB_TOKEN=... bash <(curl ...)
```

Other ways to install:

```bash
kariz-manager install --version v0.5.1        # a specific release
kariz-manager install --binary ./kariz        # a binary you built yourself
```

## How tunnels are stored

Each tunnel has a name. Its settings are `/etc/kariz/<name>.toml` (readable by root
only, since it holds the token), and systemd runs it as the service `kariz@<name>`. So
one server can run several tunnels, each started at boot and restarted if it stops.

## Add a tunnel

In the menu, choose **New tunnel**. It goes through six steps. Choices are numbered lists:
type the number (or the word), or press Enter for the default in brackets. Ctrl+C at any
point cancels and goes back to the menu, with nothing written.

| Step | Asks |
|---|---|
| 1. Name | letters, digits, `-` and `_` |
| 2. This server's side | entry (users connect here) or exit (reaches the targets); reverse (the exit connects to the entry) or direct |
| 3. Transport and profile | with a one-line hint for each |
| 4. Connection | the listening side: a port (checked to be free), IPv4 and IPv6 or IPv4 only, and the address the other server connects to (this server's IPv4 or IPv6, or another address or domain). The dialing side: the other server's address |
| 5. Security | a new token, or paste the other server's; the WebSocket path (`ws` / `wss`); the certificate pin (`wss` dialers) |
| 6. Ports to forward | entry only: the protocol, the target host as the exit reaches it (default `127.0.0.1`), and the ports. Repeat for another protocol or target |

Then it shows a summary and asks before creating anything.

### Ports

Ports are a list, separated by commas:

| Write | Means |
|---|---|
| `443` | port 443 here, to port 443 on the target |
| `443,8443,2083` | several ports |
| `8080-8090` | a range: each port to the same port on the target |
| `2053=53` (or `2053:53`) | users connect to 2053 here; the exit dials 53 |
| `3000-3005=4000-4005` | a range onto another range of the same size |
| `5000-5010=443` | many ports onto one |

They mix: `443, 8080-8090, 2053=53`. The script checks that no port is listed twice and
that none is already in use on the server. A tunnel can forward up to 1,000 ports; each
becomes one `[[forward]]` rule in its config.

### IPv4 and IPv6

Everywhere an address is asked for, IPv4, IPv6 and domains all work, with or without a
port: `203.0.113.5`, `203.0.113.5:3080`, `2001:db8::1`, `[2001:db8::1]:3080`,
`tunnel.example.com`. Without a port, the tunnel's default 3080 is used. IPv6 addresses
are written with brackets in the config.

On a server with IPv6, the tunnel and the forwarded ports listen on `[::]` by default,
which takes both IPv4 and IPv6. Choose "IPv4 only" (or `--ipv4-only`) to listen on
`0.0.0.0` instead. The two servers can talk over IPv6 while users connect over IPv4, or
the other way round.

### The other server

Before anything starts, the script writes the file and runs `kariz check` on it. If Kariz
rejects it, the error is shown and nothing is changed. Then it prints the command for the
other server, ready to paste:

```text
On the other server, run:

  kariz-manager add main --role exit --mode reverse --transport tcpmux \
      --profile balanced --token <the token> --remote 203.0.113.5:3080
```

Run that on the other server (after installing there) and the pair is up. The same
`add` command works without the menu, for automation:

```bash
kariz-manager add main --role entry --mode reverse --transport tcpmux \
    --listen 3080 --ports 443,8080-8090,2053=53
kariz-manager add games --role entry --mode reverse --transport kcp --profile gaming \
    --listen 3081 --ports 27015-27020 --protocol udp --to 10.0.0.5
```

| Option | Meaning |
|---|---|
| `--role entry\|exit`, `--mode reverse\|direct` | as in the config |
| `--transport` | `tcp`, `tcpmux`, `ws`, `wss`, `quic` or `kcp` |
| `--profile` | `balanced` (default), `ultraspeed` or `gaming` |
| `--listen PORT` / `--remote ADDR[:PORT]` | the listening side gives `--listen` (a port, or `ADDR:PORT`), the dialing side `--remote` |
| `--ports LIST` | entry only: ports as in [Ports](#ports); may repeat |
| `--protocol tcp\|udp\|tcp+udp` | for `--ports` (default `tcp`) |
| `--to HOST` | the target host for `--ports`, as the exit reaches it (default `127.0.0.1`) |
| `--ipv4-only` | listen on IPv4 only |
| `--forward LISTEN=TARGET[/tcp\|udp\|tcp+udp]` | one rule written in full, may repeat |
| `--token` | default: a new random token |
| `--ws-path`, `--pin` | `ws` / `wss`; `--pin` for a `wss` dialer |
| `--public-ip` | the address to print in the other side's command (default: this server's) |

### `wss`

Add the **listening** side first. The script makes a self-signed certificate for it
(`/etc/kariz/<name>.crt` and `.key`) and puts the certificate's pin in the command it
prints for the dialing side, so nothing has to be copied by hand.

### Open the port

The script does not touch the firewall. It reminds you which port to open: TCP for
`tcp`, `tcpmux`, `ws` and `wss`, UDP for `quic` and `kcp`, and the forwarded ports on the
entry. Cloud providers' security groups need the same rules.

## Manage tunnels

The top of the menu shows the installed version and how many tunnels run. The actions on
a tunnel (start and stop, logs, speed test, edit, remove) list the tunnels by number:
type the number or the name.

**Ctrl+C** during an action (a question, a log, a speed test) goes straight back to the
menu. At the menu itself, it leaves the manager (so does `0`).

```bash
kariz-manager list                  # name, side, transport, ports, state, address
kariz-manager status main           # connection, round trip, traffic per port
kariz-manager status                # the same for every tunnel
kariz-manager logs main             # follows the log; Ctrl-C to stop
kariz-manager speedtest main        # speed, latency and UDP; on the entry server
kariz-manager restart main          # also: start, stop
kariz-manager edit main             # opens $EDITOR (nano, else vi)
kariz-manager remove main
```

- **`status`** runs `kariz status` for the tunnel (on either server; `--watch` passes
  through), or for every tunnel without a name. A stopped tunnel shows systemd's view
  instead, with the reason it stopped. See [Status](status.md).
- **`speedtest`** runs `kariz speedtest` for the tunnel (options pass through, e.g.
  `--seconds 5`). It works on the entry server, while the tunnel runs; see
  [Speed test](speedtest.md).
- **`stop`** also disables the tunnel at boot; **`start`** turns it back on.
- **`edit`** checks your change with `kariz check` before applying it. If the check fails,
  the tunnel keeps its old settings. After a good edit it restarts the tunnel.
- **`remove`** stops the tunnel and deletes its config, certificate and token. Add
  `--yes` to skip the question.

## Update and uninstall

```bash
kariz-manager update                # latest release; running tunnels are restarted
kariz-manager update --version v0.5.1
kariz-manager uninstall             # asks before removing configs; --yes removes everything
```

`update` checks the new release's SHA-256 like `install` does, and restarts every running
tunnel so it uses the new binary. It does not replace the manager script itself; for a
new version of the manager, fetch it again (with the token while the repository is
private):

```bash
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -o /usr/local/bin/kariz-manager \
    https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh
chmod +x /usr/local/bin/kariz-manager
```

## Logs

Kariz logs go to the journal, with systemd's priority for each line, so:

```bash
journalctl -u kariz@main -f                 # follow
journalctl -u kariz@main -p warning         # only warnings and errors
```

## Good to know

- It needs root, systemd, and `curl` or `wget`. `wss` also needs `openssl`.
- The menu and the commands do the same things; use whichever suits.
- Configs written by the script are ordinary Kariz configs. Edit them by hand, and see
  the [configuration reference](configuration.md) for every setting.
- The service unit is `systemd/kariz@.service`. The older `systemd/kariz.service` (one
  tunnel, `/etc/kariz/config.toml`) still works for a hand-made setup.
