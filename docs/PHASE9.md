# Phase 9 plan: the web panel, design

Target: no release. Phase 9 designs the web panel; phases 10 to 13 build it (see
[the panel plan](#3-the-panel-plan-phases-10-to-13)) and end with **v1.0.0**.

## 1. Why

- **One place for many servers.** Today each server is set up over SSH with the manager
  script, and a tunnel means running a command on both sides by hand. The panel connects
  servers to each other, creates a tunnel on both sides at once, and shows whether it
  works.
- **Seeing the tunnel.** `kariz speedtest` and the logs are all there is today. The panel
  shows live traffic, connections and errors per tunnel and per forwarded port.
- **Its own look.** The panel should be a pleasure to use, with a visual language of its
  own, distinct from XRayMesh's.

The manager script stays: it installs Kariz and manages tunnels from a terminal, and the
panel reads and writes the same files (`/etc/kariz/<name>.toml`, `kariz@<name>`), so
the two work side by side.

## 2. Design: "Kariz at night"

The name is the idea. A kariz (qanat) carries water underground from well to well, out of
sight, to where it is needed: that is a tunnel. The panel draws a cross-section of the
earth at night in the desert.

- **Above the horizon:** a deep navy night sky with a few faint stars. The horizon is a
  thin saffron line.
- **On the horizon, the wells:** each server is a well shaft.
- **Below, the channels:** each tunnel is an underground channel between two wells, and
  water particles flow through it. Their speed and density are the tunnel's live
  traffic. This map is the home page, not a side feature.
- **Colour roles:** saffron gold leads the interface (actions, focus, selection).
  Turquoise means water only: live traffic and health. Red and amber are kept for errors
  and warnings.
- **Strata instead of cards:** panels are horizontal bands like layers of earth,
  separated by gold hairlines, with a faint grain. Numbers are set large.

| | XRayMesh | Kariz |
|---|---|---|
| Accent | turquoise | saffron gold; turquoise only for traffic |
| Layout | cards and tabs | strata bands, a live map as home |
| Type | Inter + Vazirmatn | Space Grotesk + Estedad + IBM Plex Mono, self-hosted |
| Icons | Lucide | Phosphor (duotone) plus a few qanat icons of our own |
| Charts | Recharts | uPlot (small, built for live data) |
| Navigation | header and tabs | side rail (mirrored in RTL) and a `Ctrl+K` command palette |

**Signature motion:**

- **Loader:** a drop falls down a well shaft, runs along the channel and leaves as the
  logo's arrow. At start-up the logo draws itself stroke by stroke and its channel fills.
- **Login:** a full-screen desert night. On success the view descends the well shaft and
  the dashboard rises from underground; a wrong password ripples the water.
- **Small moments:** starting a tunnel fills its channel with water, stopping it drains
  the channel; numbers roll like an odometer.
- **Less motion:** with the browser's reduced-motion setting, or the panel's low-power
  switch, particles stop and transitions become fades. The panel may be opened over a
  slow link, so the whole app stays under about 400 KB compressed.

**Themes:** "Night" (dark) is the default. "Dawn" (light: sand paper, ink navy text,
darker gold and turquoise for contrast) is for daylight and bright phone screens. Both
come from the same tokens, so the second theme costs little.

**Languages:** Persian (RTL) and English (LTR), with Persian or Latin digits as a
setting. Estedad covers Arabic script only (by `unicode-range`), so IPs, versions and
ports stay in Latin type.

The design system (tokens, type, spacing, motion timing) is in
[design/README.md](../design/README.md).

*Status after 9.1:* [design/README.md](../design/README.md) sets out the metaphor, colour
roles, type, strata layout, motion and accessibility rules; [design/tokens.css](../design/tokens.css)
holds the tokens for both themes. Every text colour reaches 4.5:1 on the three main
surfaces of its theme (measured, table in the design document). The fonts are
self-hosted in `design/fonts/` with their licences: 180 KB in all, most of it Estedad,
which phase 11 will subset.

*Status after 9.2:* the prototype is in [design/prototype/](../design/prototype/): plain
HTML, CSS and JavaScript with no dependencies, served by any static server from the
repository root (`python -m http.server`, then `/design/prototype/`). It has:

- the boot: the logo draws itself and its channel fills, then the login;
- the login scene on a canvas: stars that twinkle, a shooting star now and then, dunes
  with a little parallax, three wells and the channel with flowing water; a wrong password
  shakes the field and ripples the water red; the right one (`kariz` in the prototype)
  runs the descent into the dashboard;
- a one-time link: `#t=...` in the address signs in by itself, and the fragment is
  removed from the address bar at once; the "I have a one-time link" button shows the
  same flow with the qanat loader;
- the dashboard: the side rail (a bottom bar on phones), the sky strip with `Ctrl+K`, the
  live map (five servers, five tunnels: flowing, broken with a pulse at the break, and
  stopped), key numbers that roll like an odometer, and the tunnel table with sparklines;
  switching a tunnel off drains its channel, on fills it;
- Night and Dawn, Persian (RTL, Persian digits) and English (LTR), the low-power switch
  (also on with the browser's reduced-motion setting), the command palette, toasts.

Checked at 1440, 800 and 375 px wide in both themes and both languages: no horizontal
scroll, labels at the map's edges stay on screen. Other pages show the loader and a note
that they come in 9.3.

*Status after 9.3:* the prototype now has every screen: servers and adding a server with
a join code, tunnels, tunnel detail (with a diagnosis for a broken tunnel), the pair
wizard (create and edit), logs and settings. It is split into `app.js` (boot, login, the
map, the shell), `pages.js` (the screens) and `wizard.js`; [design/README.md](../design/README.md#9-screens)
lists what each screen settles. Checked at 1440, 800 and 375 px, in both themes and both
languages, with no script errors and no horizontal scroll. Things the design had to
settle along the way:

- **Addresses in Persian text.** IPv6 addresses, paths and `kariz@name` came out
  reordered inside Persian sentences; they are now isolated as left-to-right text, in the
  page and on the canvas.
- **Wide code.** A config or a command widened its column or dialog; code blocks now
  scroll inside themselves.
- **Port conflicts name their owner.** "443 is in use" is not enough; the wizard says
  which tunnel (or service, like `sshd`) holds it on which server.

## 3. The panel plan (phases 10 to 13)

### Architecture

```
browser --HTTPS--> kariz-panel serve   <-- Kariz's own protocol -->  kariz-panel agent (each server)
                   axum + SQLite                                      |- /etc/kariz/<name>.toml
                   web app inside the binary                          |- systemd kariz@<name>
                                                                      '- kariz control socket
```

- **Backend in Rust:** a new crate in this repository, one binary `kariz-panel` with two
  modes, `serve` and `agent`. SQLite is bundled, so the binary stays static and ships for
  the same three architectures.
- **Connecting servers:** the panel makes a one-line join command with a join code; run
  on another server, it installs the agent and connects it. The management link runs on
  Kariz's own protocol (token handshake and mux over `tcpmux` or `wss`), so it resists DPI
  like the tunnels do. Both directions work; by default the agent dials the panel, so the
  server needs no extra open port.
- **No remote shell:** the agent accepts a fixed list of operations: write a tunnel
  config (checked by `kariz check` first), start / stop / restart, read status and logs,
  run a speed test, update the binary (SHA-256 checked).
- **A tunnel is a pair:** pick the entry and exit servers; the panel makes the token and
  both configs, sends them, starts them in the right order and checks that they connect.
  Tunnels made by the manager script are imported.

### Sign-in

A one-time login link plus an optional fixed password, as in XRayMesh, with its weak
points fixed:

| XRayMesh | Kariz |
|---|---|
| salted SHA-256 password hash | Argon2id |
| token in `?token=` (lands in logs and history) | token in the fragment (`#t=...`), never sent to the server; removed from the address bar after sign-in |
| sessions in memory, lost on restart | sessions in SQLite, listed and revocable in the panel |
| | a secret path (`/k-xxxx/`), a random port, rate limits, `Secure` + `SameSite=Strict` cookies, CSRF protection, optional TOTP |

`kariz-panel login-link` (over SSH) makes a link valid for 60 minutes;
`kariz-panel reset-password` is the way back in. TLS is self-signed (its fingerprint is
shown) or Let's Encrypt when there is a domain.

### Install

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
```

The manager script gains a "Web panel" entry: it downloads `kariz-panel` with `kariz`
(SHA-256 checked), installs its service and prints the panel address and a login link.
`--agent <code>` connects a server to a panel. While the repository is private the
download still needs `GITHUB_TOKEN`.

### Phases

| Phase | Scope | Release |
|---|---|---|
| **9** | Design: system, interactive prototype, all screens (this document) | none |
| **10** | Core: counters, `status` on the control socket on both sides, `kariz status`, `kariz check --json` | v0.7.0 |
| **11** | Panel base: crate, sign-in, installer, agents and servers, app shell | v0.8.0-beta |
| **12** | Tunnels (pair wizard, edit, import) and live monitoring (map, charts, logs, speed test, updates, backup) | v0.9.0 |
| **13** | Playwright tests, security review, accessibility, docs in both languages | v1.0.0 |

Each phase gets its own plan document when it starts.

## 4. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **9.0** Plan (done) | This document. | |
| **9.1** Design system (done) | Tokens (colour for both themes, type, spacing, radius, motion), fonts, RTL rules, in `design/`. | Document and `tokens.css` in the repository. |
| **9.2** Prototype (done) | Interactive HTML: loader, login, the map with live-looking traffic, both themes, both languages. | Approved. |
| **9.3** Screens | Servers, tunnels, the pair wizard, tunnel detail, logs, settings, mobile. | Approved. |
