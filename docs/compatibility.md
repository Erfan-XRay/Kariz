# Compatibility from 1.0

Kariz follows [semantic versioning](https://semver.org/) from 1.0.0: what works with 1.0 keeps
working with every 1.x, and a change that would break it waits for 2.0. Before 1.0 nothing was
promised, and things were changed freely.

This page says what "works" covers, and what is left free.

## What 1.x keeps

| What | The promise |
|---|---|
| **Tunnel configuration** (`/etc/kariz/*.toml`) | A config that is valid for 1.0 is valid for every 1.x, and means the same. New settings are optional and have defaults that keep today's behaviour. A setting is renamed only by keeping the old name working (as `throughput` still works for `ultraspeed`), and removed only in 2.0, after `kariz check` has warned about it for at least one release. |
| **The tunnel protocol** (handshake, records, mux, transports) | Any 1.x entry works with any 1.x exit, in either direction, on every transport. A new feature that needs both sides (a new transport, a new option of the wire) is used only when both sides have it. |
| **The status document** (`kariz status --json`, the control socket) | `status_version` stays `1`: fields are only added, never renamed or removed. Read it by name and ignore what you do not know. |
| **`kariz-manager`** | Its commands (`install`, `update`, `uninstall`, `panel ...`, `agent ...`, `--agent`) and options keep their names and meanings; the menu may change. Tunnels are made in the web panel, not in the manager. The configs and services it makes (`/etc/kariz/NAME.toml`, `kariz@NAME`, `kariz-panel`, `kariz-agent`) keep their names and places. |
| **The panel and its agents** | An agent of any 1.x connects to a panel of any 1.x. What a panel asks that an older agent does not know is answered with "unknown request", and the panel shows that server as needing an update (which the panel can then do: every 1.x agent can update itself). The panel's own database is migrated forward by each version, so a newer panel opens an older database. |
| **Releases** | Archives keep their names (`kariz-vX.Y.Z-ARCH-linux.tar.gz`), their checksum file and their signature, and the release key stays the same for all of 1.x. |

## What is not promised

- **The panel's HTTP API** (`/api/...`) is not a public interface: the web app and the panel are
  released together and the API changes with them. Do not build tools on it. (If you need to
  automate, use the tunnel configs, which are stable.)
- **Going back.** A newer panel migrates its database forward; an older one cannot read it. To go
  back, restore a backup made before updating.
- **Log lines and messages** are for people and change without notice. The status document and
  the exit codes are what to read from a program.
- **Performance numbers** and the exact defaults of tuning values (window sizes, timeouts) may
  improve in a minor release. The behaviour a profile stands for stays.
- **Features marked experimental** in the documentation (for example `quic.congestion = "bbr"`).
- **The look of the panel** and the wording of its texts.

## How a breaking change is made

1. It is announced in the CHANGELOG as *deprecated*, and `kariz check` (or the panel) warns
   wherever it applies, at least one minor release before it goes.
2. It goes only in a major release (2.0), which says in its CHANGELOG what to change.
3. A security fix may break compatibility in a minor release when there is no other way; the
   CHANGELOG then says so at the top of the entry.

## Keys and signing

The release key built into the panel and the manager is the same for all of 1.x. If it ever has to
change (a leak), the new key comes in a release signed by the old one where possible, and the
CHANGELOG explains what to do by hand where it is not.
