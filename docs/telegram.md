# Telegram alerts and status

The panel can send a message in Telegram when a server goes offline or a tunnel goes down, and
tell you when it is back. The same bot answers `/status` and a few other commands, so you can
look at every server and tunnel from your phone. It is set up in the panel, under
**Settings > Telegram**.

The bot is your own: you make it, the token stays in your panel, and no one else's server is
in between (Telegram's, of course, is).

## Set it up

1. In Telegram, talk to [@BotFather](https://t.me/BotFather), send `/newbot` and follow it. It gives you a
   **token** that looks like `123456789:AAH...`.
2. In the panel, open **Settings > Telegram**, paste the token and press *Save the bot*. The
   panel keeps it, never shows it again (only its last four characters) and turns the bot on.
3. Press *Connect a chat*. The panel shows `/start CODE`. Open your bot in Telegram and send
   exactly that. The code works once and for ten minutes. The chat is connected and the bot says
   so. A group works the same way: add the bot to the group and send the code there.
4. Press *Send a test message*. It says the bot's name when the token and the connection are good,
   or why not.

Only connected chats are answered. Anyone else who finds your bot gets no reply at all, whatever
they send. *Disconnect* removes a chat. A different token is a different bot, so the chats of the
old one are dropped.

## What it tells you

| Message | When |
|---|---|
| 🔴 Server *name* went offline | a server stayed offline for the wait time (60 seconds); the tunnels that run on it are named in the same message |
| 🟢 Server *name* is back | it came back, with how long it was offline |
| 🔴 Tunnel *name* is down | a tunnel stayed down for its wait time (30 seconds); the last error is shown |
| 🟢 Tunnel *name* is up again | it is connected again, with how long it was down |

So that you are not woken up for nothing:

- **A wait before telling.** A restart or a blip that heals inside the wait says nothing at all.
  Both waits can be changed (5 to 3600 seconds).
- **Quiet while the panel starts.** For the first minute after the panel itself starts,
  everything looks offline for a moment, so nothing is sent. What is still down after that is told
  once.
- **A server's tunnels are not told one by one.** When a server is offline its tunnels cannot be
  read, so one message names them all.
- **A tunnel that keeps going down** (more than four times in an hour) gets one message saying it
  is unstable, and then its messages stop until an hour has passed with fewer downs.
- **A message that cannot be sent** (Telegram unreachable for a moment) is tried again for a
  while, with longer waits, and kept in order.

*Tell me when a server goes offline* and *a tunnel goes down* can be turned off separately. The
language of the messages (Persian or English) is a setting here, not the panel's.

## Ask the bot

| Command | Answer |
|---|---|
| `/status` | every server and tunnel at a glance: up or down, CPU and memory, tunnel speed and round trip |
| `/servers`, `/server NAME` | the servers; one in detail (load, uptime, version, addresses, tunnels) |
| `/tunnels`, `/tunnel NAME` | the tunnels; one in detail (role, how long connected, sessions, traffic, last error) |
| `/mute 2h` | silence the alerts for a while (`30m`, `2h`, `1d`; up to 30 days) |
| `/unmute` | turn them back on |

A name can be shortened to where it is not ambiguous. The commands only read: nothing here can
change, stop or restart a tunnel.

A **status report** can be sent by itself every 6, 12 or 24 hours (off by default): it is the
same message as `/status`.

## When Telegram is blocked from the panel's server

Telegram cannot be reached from some countries. The panel's server needs to reach
`api.telegram.org`. If it cannot, open *If Telegram is blocked from this server* and use **one** of:

- **A proxy**: `http://host:port` or `socks5://host:port`, with `user:password@` before the host
  if it needs them. A proxy you already have on that server (an Xray or V2Ray client that opens a
  local SOCKS port, for example) works: `socks5://127.0.0.1:10808`.
- **A local port that a tunnel forwards**: if the panel's server is the *entry* of a Kariz tunnel
  to a server where Telegram is not blocked, add a forward to that tunnel's config:

  ```toml
  [[forward]]
  listen = "127.0.0.1:8443"
  target = "api.telegram.org:443"
  ```

  and write `127.0.0.1:8443` in *Through a local port*. The exit server resolves and dials
  `api.telegram.org`; the panel still checks Telegram's own certificate for that name, so the
  forwarding server sees only encrypted bytes.
- **Another API address**: a relay you run yourself (for example nginx on a server that can reach
  Telegram) that passes requests to `api.telegram.org`. Write its `https://` address. The token
  goes through that relay, so only use one you control.

The *Send a test message* button shows what failed, so you can tell a wrong proxy from a blocked
address.

## Security

- The token is stored in the panel's database. It is **not** in a backup (see
  [Backup](panel.md#backup-and-restore)): enter it again after restoring.
- The settings page and the API never return the token, and errors that mention a Telegram address
  have the token removed before they are shown or logged. A proxy's password is shown as `***`.
- Only chats connected with a one-time code are answered, and only the panel's signed-in user can
  make a code.
- A message names your servers and tunnels, so treat the chat like the panel: a group the bot is in
  can read them.

## Troubleshooting

| What you see | Cause |
|---|---|
| *Unauthorized* | the token is wrong or the bot was deleted; make a new token in @BotFather |
| *Connection failed*, a timeout | Telegram is not reachable from the panel's server: see the section above |
| The test works but no alerts come | no chat is connected, the kind of message is off, the alerts are muted (`/unmute`), or the thing was not down for the whole wait |
| `/start CODE` gets no answer | the code was used or has expired (ten minutes): press *Connect a chat* for a new one |
| *chat not found* or *bot was blocked by the user* | the chat removed the bot: connect it again |
| *Conflict: can't use getUpdates method while webhook is active* | the bot has a webhook set elsewhere (another program uses it): use a bot of its own for Kariz |
