# Running Kariz behind a CDN

A CDN hides the tunnel server behind the CDN's own IP addresses. The DPI only sees a
TLS connection to a CDN edge, with the SNI of an ordinary website, carrying a
WebSocket. This page covers Cloudflare (tested first) and has notes for ArvanCloud.
The manual checklist at the end is the acceptance test for step 2.6 of
[PHASE2.md](PHASE2.md): nginx stands in for the CDN in CI, but a real CDN has to be
checked by hand.

## Which side sits behind the CDN

The CDN sits in front of the **listening** side, and the dialing side connects to a
CDN edge.

| Mode | Dials | Behind the CDN | Typical CDN |
|---|---|---|---|
| `direct` | entry (Iran) | exit (abroad) | Cloudflare |
| `reverse` | exit (abroad) | entry (Iran) | a CDN with edges inside Iran, e.g. ArvanCloud |

## Configuration

Direct mode through Cloudflare, with the exit serving `wss` to Cloudflare
(SSL/TLS mode **Full (strict)**):

```toml
# entry (Iran): dials a Cloudflare edge
role = "entry"
mode = "direct"

[[forward]]
listen = "0.0.0.0:443"
target = "127.0.0.1:443"

[tunnel]
transport = "wss"
remote = "104.16.0.1:443"          # a clean Cloudflare IP (or tunnel.example.com:443)
token = "..."

[tunnel.ws]
path = "/api/v1/stream"
host = "tunnel.example.com"        # also used as the TLS server name
early_data = true                  # hello inside the upgrade request: one round trip less
```

```toml
# exit (abroad): the origin behind Cloudflare
role = "exit"
mode = "direct"

[tunnel]
transport = "wss"
listen = "0.0.0.0:443"
token = "..."

[tunnel.ws]
path = "/api/v1/stream"
host = "tunnel.example.com"        # optional: reject requests for other hosts

[tunnel.tls]
cert = "/etc/kariz/origin.pem"     # Cloudflare Origin CA or Let's Encrypt certificate
key = "/etc/kariz/origin.key"
```

Alternative: the exit serves plain `ws` (e.g. on port 80 or 8080) and Cloudflare uses
SSL/TLS mode **Flexible**. The entry still uses `wss` to the edge. The leg from
Cloudflare to the origin is then not TLS, but the tunnel's own encryption still covers
the traffic.

Notes:

- **Do not use `tls.pin_sha256` towards a CDN.** The entry sees the CDN's edge
  certificate, which is publicly trusted and rotates often. The default verification
  (Mozilla roots) is the right one there. Pinning is for connecting straight to your
  own server with a self-signed certificate.
- **Mux stays on** (the default for `ws` / `wss`). It pings every `keepalive` (30 s by
  default), which keeps the WebSocket from being cut as idle: Cloudflare cuts idle ones
  after about 100 s. `kariz check` warns if `tuning.keepalive_secs` is above 90. Without
  mux, a quiet user connection is cut by the CDN.
- **Ports Cloudflare proxies:** HTTPS 443, 2053, 2083, 2087, 2096, 8443; HTTP 80, 8080,
  8880, 2052, 2082, 2086, 2095.
- Probes that reach the origin through the CDN get nginx's `404` page for every path
  but `ws.path`, and also for an upgrade on `ws.path` whose early data does not hold a
  valid hello.

## Manual checklist (Cloudflare)

Setup:

1. DNS: an `A` record `tunnel.example.com` → exit IP, **Proxied** (orange cloud).
2. SSL/TLS → Overview: **Full (strict)** for a `wss` origin (**Flexible** for `ws`).
3. Network: **WebSockets** on (the default).
4. The origin port is one Cloudflare proxies (list above).
5. Optional: the origin firewall only accepts Cloudflare's IP ranges.
6. `kariz check -c <config>` on both sides: `config OK`, no warnings.

Checks (tick each, and note the date and Kariz commit):

- [ ] `curl -sI https://tunnel.example.com/` → `404`, `server: cloudflare`.
- [ ] `curl -s https://tunnel.example.com/api/v1/stream` (no upgrade) → `404`.
- [ ] Start the exit, then the entry. The exit logs `mux session from the entry side
      established` once per `mux.connections`, and the entry logs no reconnects.
- [ ] With `RUST_LOG=kariz=debug` on the exit: `websocket upgrade request accepted`
      shows `client=<entry IP>` (from `CF-Connecting-IP`) and `early=` above 0.
- [ ] Traffic through a forward port works: a large download, plus many parallel
      short connections (for example a browser through a proxy served on the exit).
- [ ] Idle: open a user connection, leave the tunnel idle for 5 minutes, then use the
      same connection again. It still works and no reconnects are logged.
- [ ] Restart the exit: the entry reconnects within a few seconds and new connections
      work.
- [ ] With `mux.max_lifetime_secs = 600`: sessions are replaced every ~10 minutes and
      open connections are not broken.
- [ ] Wrong `ws.path` on the entry: it logs `websocket upgrade refused with HTTP 404`.

Troubleshooting:

| Symptom | Likely cause |
|---|---|
| Entry: `upgrade refused with HTTP 404` | `ws.path` / `ws.host` differ, or (with early data) the hello was rejected: token mismatch, clocks out of sync, see the exit log. |
| Entry: HTTP `521` / `522` | Origin down, or the port is not reachable from Cloudflare. |
| Entry: HTTP `525` / `526` | Full / Full (strict): the origin does not serve TLS, or its certificate is not valid for the name. |
| Entry: certificate error | Pinning towards a CDN (remove `pin_sha256`), or `tls.sni` / `ws.host` not the proxied name. |
| Disconnects about every 100 s | Mux off, or `keepalive_secs` above ~90. |

## ArvanCloud (reverse mode)

For CDNs with edges inside Iran the same settings apply, with the roles swapped: the
entry (Iran) is the origin behind the CDN, and the exit dials the CDN's edge. Check the
CDN's WebSocket support and its idle timeout. The checklist above applies with "entry"
and "exit" exchanged. *Not yet verified; results to be added here.*

## Results

| Date | CDN | Mode | Transport (edge / origin) | Early data | Result | Notes |
|---|---|---|---|---|---|---|
| | Cloudflare | direct | wss / wss | on | | |
| | Cloudflare | direct | wss / ws | on | | |
| | ArvanCloud | reverse | wss / wss | on | | |
