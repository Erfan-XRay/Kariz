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

In the menu, choose **New tunnel**. It asks, in this order:

| Question | Notes |
|---|---|
| Name | letters, digits, `-` and `_` |
| Entry or exit | the entry is the server users connect to |
| Reverse or direct | who dials whom (see [Getting started](getting-started.md)) |
| Transport, profile | with a one-line hint for each transport |
| Port or the other server's address | the listening side asks for a port and its public IP, the dialing side for the other server's address |
| Token | a new one, or paste the other server's |
| WebSocket path | `ws` and `wss` only, random by default |
| Certificate pin | `wss` dialers only |
| Forward rules | entry only: `443=127.0.0.1:443`, or `51820=127.0.0.1:51820/udp` |

A forward rule is `LISTEN=TARGET[/tcp|udp|tcp+udp]`; a bare port listens on all
addresses, and the target is dialed from the exit.

Before anything starts, the script writes the file and runs `kariz check` on it. If Kariz
rejects it, the error is shown and nothing is changed.

Then it prints the command for the other server, ready to paste:

```text
On the other server, run:

  kariz-manager add main --role exit --mode reverse --transport tcpmux \
      --profile balanced --token <the token> --remote 203.0.113.5:3080
```

Run that on the other server (after installing there) and the pair is up. The same
`add` command works without the menu, for automation:

```bash
kariz-manager add main --role entry --mode reverse --transport tcpmux \
    --listen 0.0.0.0:3080 --forward 443=127.0.0.1:443 --forward 8080=127.0.0.1:80
```

| Option | Meaning |
|---|---|
| `--role entry\|exit`, `--mode reverse\|direct` | as in the config |
| `--transport` | `tcp`, `tcpmux`, `ws`, `wss`, `quic` or `kcp` |
| `--profile` | `balanced` (default), `ultraspeed` or `gaming` |
| `--listen ADDR:PORT` / `--remote ADDR:PORT` | the listening side gives `--listen`, the dialing side `--remote` |
| `--token` | default: a new random token |
| `--forward` | entry only, may repeat |
| `--ws-path`, `--pin` | `ws` / `wss`; `--pin` for a `wss` dialer |
| `--public-ip` | the address to print in the other side's command (default: this server's) |

### `wss`

Add the **listening** side first. The script makes a self-signed certificate for it
(`/etc/kariz/<name>.crt` and `.key`) and puts the certificate's pin in the command it
prints for the dialing side, so nothing has to be copied by hand.

### Open the port

The script does not touch the firewall. It reminds you which port to open: TCP for
`tcp`, `tcpmux`, `ws` and `wss`, UDP for `quic` and `kcp`. Cloud providers' security
groups need the same rule.

## Manage tunnels

In the menu, the actions on a tunnel (start and stop, logs, speed test, edit, remove)
list the tunnels by number: type the number or the name. Ctrl-C leaves a log and returns
to the menu.

```bash
kariz-manager list                  # name, side, transport, state, address
kariz-manager status main
kariz-manager logs main             # follows the log; Ctrl-C to stop
kariz-manager speedtest main        # speed, latency and UDP; on the entry server
kariz-manager restart main          # also: start, stop
kariz-manager edit main             # opens $EDITOR (nano, else vi)
kariz-manager remove main
```

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
tunnel so it uses the new binary.

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
