# Using the panel

This page walks through the panel in the order you need it, from a new server to a tunnel you
can trust. You do not need any other tutorial: everything about servers and tunnels is done here,
in the browser. Try the same screens first, with sample data and nothing to install, in the
[live demo](https://erfan-xray.github.io/Kariz/try/), take the
[guided tour](https://erfan-xray.github.io/Kariz/docs/tour/), which opens each screen as it explains it, or
watch the [video tutorial](https://erfan-xray.github.io/Kariz/docs/video/) (about two minutes, from a bare server to a running tunnel).

## 1. Sign in

On the server that holds the panel, the manager printed an address and a one-time link. Open the
link: it signs you in and vanishes from the address bar. The certificate is a real Let's Encrypt one, so
the browser shows no warning.

- Want a password instead of links? *Settings*, set one (10 characters or more).
- Lost the link? On the server, `kariz-manager panel link` makes another.

## 2. Add your servers

Open **Servers**, then **Add server**.

1. Give the server a name, and check the address (the panel's address as that server reaches it).
2. Leave the **link transport** on *Auto*, or pick one: *TCP*, *KCP*, *WSS* or *QUIC*. This is how the
   server talks to the panel, not how tunnels work. *Auto* tries them in turn and keeps the one that
   gets through. *QUIC* is always sealed, so it is the one to try where plain QUIC is filtered; it
   needs UDP open on the panel's port after the agents port.
3. The panel shows a **join code** and a command. On the other server, as root, run it:
   `kariz-manager --agent kz1_...`
4. Within seconds the server appears in the list, with its CPU, memory and network.

The new server opens no port: it connects to the panel. Codes work once, for 10 minutes.

## 3. Make your first tunnel

Open **Tunnels**, then **New tunnel**. The wizard asks five short things and explains the choice you make:

| Step | You choose |
|---|---|
| **Servers** | the *entry* (where your users connect) and the *exit* (which reaches the real services); the name is made from the two, change it if you like |
| **Transport** | the direction, *reverse* (the exit dials the entry; the exit opens no port) or *direct*; the transport; the profile |
| **Address** | the port the accepting side listens on, and the address the other side reaches it at |
| **Ports** | what the entry opens for your users: `443, 8080-8090, 2053=53`, TCP, UDP or both |
| **Build** | a review, then the panel builds both sides and shows every step |

What most tunnels never change waits under **More options** at the foot of a step, with a line
that says what is set there now: the encryption, sealed QUIC (on by default), mux and its settings
on *Transport*; the WebSocket path and Host, the TLS server name and idle connections on
*Address*; a limit of connections per user address, UDP clients per port and *Real user IPs
(PROXY protocol)* so Xray sees each user's address (only with `acceptProxyProtocol` on in Xray,
see [configuration](configuration.md#real-user-addresses-proxy_protocol)) on *Ports*.

The wizard picks the port for you: 3080 for the first tunnel on a server, then 3081, 3082 and so on, the
first one that none of that server's tunnels holds. You can type another.

Not sure what to pick? The default (reverse, `tcpmux`, `balanced`) is right for most paths; the
[transport chooser](https://erfan-xray.github.io/Kariz/docs/how-it-works/) answers in four questions.

**If anything fails, both servers are put back as they were**, and the wizard tells you which
step failed and why. You cannot end with half a tunnel.

## 4. See what is happening

- **Map:** the overview. The key numbers (throughput, with a line of the last minutes),
  then the map: each server is a well; each tunnel is a channel, with its name and its rate
  now. Flowing water is traffic, a dashed empty channel is a stopped tunnel, a red pulse is a
  broken one. Hover for details, click a tunnel to open it. Under the map: the tunnels,
  **Needs attention** (a broken tunnel, a tunnel that is connected but lets no user through, one
  whose streams are nearly all taken, an offline server, a busy CPU, servers behind on the
  version, each with a button to go there) and every server's load.
- **The top bar** says *Live* while the panel answers. If it stops answering, a notice says
  so and the page keeps the last numbers until it is back.
- **Servers:** a card per server with its load, version and link. A server that is **offline**
  says since when and why, and has a **Reconnect** button: a new join code that makes the agent
  on it *that same server* again (same tunnels, nothing lost), with a choice of how it reaches
  the panel, if one protocol is filtered ([details](panel.md#a-server-that-goes-offline)). Click or tap its **IPv4** or
  **IPv6** address (or the address set for it) to copy it: the chip shows a check and a note says
  what was copied. On a phone each address is a full-width row, easy to hit.
- **Tunnels:** click a row. You see both sides, live charts of throughput and round trip (1 hour to
  30 days), and the ports it forwards with their traffic. **Health** says whether a new connection
  gets through right now (checked every 15 seconds, the way a user connects) and how many of the
  tunnel's streams are in use; a tunnel that is connected but lets no one through says why, with
  the fix.
- **Logs:** pick a tunnel and read both sides interleaved by time. Filter by level or text.
- **Events:** a server going offline, a tunnel losing its connection, and coming back.

### Usage

How much each tunnel and server has used, the way a provider counts it: on the network, with
everything that goes with the users' bytes.

- **A tunnel's Usage tab:** today, this week, this month and in total (since it was first
  counted), download and upload, and what it used on each of its two servers. The chart shows
  today by the hour, 7 or 30 days, this month by the day, this year by the month, or days you
  pick on a calendar (*Pick days…*: one day, or a first and a last). *Breakdown* splits it into:
  - **Users' data:** the users' bytes, counted on every leg they cross. A byte a user downloads
    crosses the entry server twice (in from the exit, out to the user), so on the entry server it
    counts twice: that is what its provider sees. A target on the same server (Xray on
    `127.0.0.1`) is reached over the loopback, which no provider counts, so that leg is left out.
  - **Tunnel overhead:** what the tunnel adds on its link: encryption, framing, handshakes,
    keepalives, health probes, UDP copies.
  - **Resends and FEC:** what was sent again (TCP retransmissions, KCP resends) and FEC parity.
    Large on a lossy line.
  - **Packet headers:** IP, TCP and UDP headers, the Ethernet header, and a GRE or VXLAN link's
    own header when the tunnel goes over one.
- **A server's card** shows this month's usage and, if it has one, its quota. **Usage** on the
  card opens its chart: the network card's totals (what the provider bills: the real cards, not
  GRE/VXLAN links, bridges or docker counted twice), how much of the month its tunnels were, and
  *Its tunnels* to chart those alone.
- **Monthly quota:** many providers give a server a volume a month (2 TB, say). Set it on the
  server's Usage: the volume, what counts (download and upload, upload only as Hetzner bills, or
  download only), the day of the month it renews on, and the share to warn at (80% by default).
  The card shows how much is used; at the warning and when it is used up, the log says so and
  [Telegram](telegram.md) tells you, once each period.
- **Reset usage** (a tunnel's tab, or a server's Usage, optionally with its tunnels) deletes what
  was counted and starts again from now. Deleting a tunnel deletes its usage; removing a server
  deletes its own and its tunnel sides'.
- **Calendar and time zone** are in *Settings → Usage*: Solar Hijri (the default) or Gregorian,
  and where a day starts (Tehran by default). Months, weeks (from Saturday in the Solar Hijri
  calendar, from Monday in the Gregorian) and quota periods follow them.

The panel keeps 15-minute sums for 90 days and daily sums for good, a few hundred kilobytes a
tunnel a year. Every reading is compared with the last one kept, so restarting the panel, a
tunnel or a server loses nothing and counts nothing twice; only what a tunnel moved in the
moments before it crashed while the panel could not see it is lost. Exact counts need Kariz
2.7.0-beta.2 on the servers; a tunnel on an older one shows the users' bytes alone, marked as an
estimate. How it is counted: [Status](status.md#wire-what-the-network-really-carried).

## 5. Change, stop, test, delete

Open the tunnel, then:

- **Edit:** the same wizard, filled in. Both sides change together; if they do not connect again,
  the old settings come back.
- **Move it to other servers:** on the edit page, *Servers* has the entry and the exit to pick (and
  *Swap*). The tunnel is set up and tested on its new servers first, and taken off a server it left
  only once both sides connect; a server that is offline removes it when it connects again. If the
  new pair does not connect, nothing changes. A side on a server that is offline can be moved this
  way too, to bring the tunnel back on another server. A move gives both sides a new token, and its
  new servers need Kariz 2.7 or newer.
- **Start, stop, restart:** stopping also keeps it stopped at boot.
- **Speed test:** measures download, upload, latency under load and UDP through the tunnel, for
  the seconds you choose.
- **New token:** replaces the secret on both sides.
- **Auto restart:** restarts the tunnel by itself, every so many minutes, hours or days or every
  day at a time. The same on a server's card restarts all its running tunnels
  ([details](panel.md#restart-on-a-timer)).
- **Delete:** removes it from both servers.

## 6. Private networks

*Networks* makes private addresses between your servers (over GRE or VXLAN) that are never repeated, so a
tunnel (direct or reverse) can run over them, and the tunnel's edit page can move it on or off a network: [Private networks](networks.md).

## 7. Keep it safe

In **Settings**: the password, one-time links, and every signed-in device with a *Revoke* button.
*Download a backup* gives one file locked with a passphrase you choose; keep the passphrase.

## 8. Update

A notice appears in the top bar when a newer release is out. **Settings, Updates** shows the
release notes and two buttons: update the panel, and update the servers (one at a time, each
checked, and put back if it does not come up). The panel never updates by itself.

## Handy things

- **Ctrl+K** opens a search for pages, commands and tunnels (type a tunnel's name to open it).
- The address follows the page, so the browser's Back button works and a bookmark opens the
  same page.
- The top bar switches **Persian / English**, **Night / Dawn**, and a **low-power** mode that
  stops the animation (also on when your browser asks for less motion).
- Everything works on a phone.

## When something is wrong

[Troubleshooting](panel.md#troubleshooting) lists the usual ones: a server that does not show up,
a port in use, a tunnel that did not connect, being locked out.
