# Using the panel

This page walks through the panel in the order you need it, from a new server to a tunnel you
can trust. You do not need any other tutorial: everything about servers and tunnels is done here,
in the browser. Try the same screens first, with sample data and nothing to install, in the
[live demo](https://erfan-xray.github.io/Kariz/try/), or take the
[guided tour](https://erfan-xray.github.io/Kariz/docs/tour/), which opens each screen as it explains it.

## 1. Sign in

On the server that holds the panel, the manager printed an address and a one-time link. Open the
link: it signs you in and vanishes from the address bar. The certificate is a real Let's Encrypt one, so
the browser shows no warning.

- Want a password instead of links? *Settings*, set one (10 characters or more).
- Lost the link? On the server, `kariz-manager panel link` makes another.

## 2. Add your servers

Open **Servers**, then **Add server**.

1. Give the server a name, and check the address (the panel's address as that server reaches it).
2. The panel shows a **join code** and a command. On the other server, as root, run it:
   `kariz-manager --agent kz1_...`
3. Within seconds the server appears in the list, with its CPU, memory and network.

The new server opens no port: it connects to the panel. Codes work once, for 10 minutes.

## 3. Make your first tunnel

Open **Tunnels**, then **New tunnel**. The wizard asks five short things and explains each one:

| Step | You choose |
|---|---|
| **Servers** | a name, the *entry* (where your users connect) and the *exit* (which reaches the real services) |
| **Kind** | *reverse* (the exit dials the entry; the exit opens no port) or *direct*; the transport; the profile |
| **Connection** | the port the accepting side listens on, and the address the other side reaches it at |
| **Ports** | what the entry opens for your users: `443, 8080-8090, 2053=53`, TCP, UDP or both |
| **Build** | a review, then the panel builds both sides and shows every step |

Not sure what to pick? The default (reverse, `tcpmux`, `balanced`) is right for most paths; the
[transport chooser](https://erfan-xray.github.io/Kariz/docs/how-it-works/) answers in four questions.

**If anything fails, both servers are put back as they were**, and the wizard tells you which
step failed and why. You cannot end with half a tunnel.

## 4. See what is happening

- **Map:** the overview. The key numbers (throughput, with a line of the last minutes),
  then the map: each server is a well; each tunnel is a channel, with its name and its rate
  now. Flowing water is traffic, a dashed empty channel is a stopped tunnel, a red pulse is a
  broken one. Hover for details, click a tunnel to open it. Under the map: the tunnels,
  **Needs attention** (a broken tunnel, an offline server, a busy CPU, servers behind on the
  version, each with a button to go there) and every server's load.
- **The top bar** says *Live* while the panel answers. If it stops answering, a notice says
  so and the page keeps the last numbers until it is back.
- **Tunnels:** click a row. You see both sides, live charts of throughput and round trip (1 hour to
  30 days), and the ports it forwards with their traffic.
- **Logs:** pick a tunnel and read both sides interleaved by time. Filter by level or text.
- **Events:** a server going offline, a tunnel losing its connection, and coming back.

## 5. Change, stop, test, delete

Open the tunnel, then:

- **Edit:** the same wizard, filled in. Both sides change together; if they do not connect again,
  the old settings come back.
- **Start, stop, restart:** stopping also keeps it stopped at boot.
- **Speed test:** measures download, upload, latency under load and UDP through the tunnel, for
  the seconds you choose.
- **New token:** replaces the secret on both sides.
- **Delete:** removes it from both servers.

## 6. Private networks

*Networks* makes private addresses between your servers (over GRE) that are never repeated, so a
direct tunnel can run over them: [Private networks](networks.md).

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
