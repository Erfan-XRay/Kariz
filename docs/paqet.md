# Paqet over raw TCP

Paqet is a Kariz tunnel transport for Linux and IPv4. It carries the same encrypted KCP
packets as [`kcp`](transports.md#kcp), but inside raw TCP segments instead of UDP. Each
packet looks like data on an established TCP connection, and there is no handshake. Use it
on routes that block or throttle UDP but let TCP through.

Kariz builds the TCP header of every segment itself and lets Linux route it. There is
nothing to install and nothing to set up by hand. The tunnel service runs as root, opens
the raw socket and adds the host firewall rules Paqet needs (see
[Firewall rules](#firewall-rules)). Both servers need Kariz 2.8.0-beta.1 or newer;
2.8.1 or newer is best (see [Versions](#versions)).

## In the panel

Pick **paqet** on the transport step of the tunnel wizard, or on a tunnel's edit page.
The rest works as for the other transports:

- **Address.** The listening server listens on every IPv4 address of its own, like the
  other transports. It answers each connection from the address that connection came to,
  so a server with several addresses works too. There is no address to fill in.
- **Port.** Any free TCP port. The wizard suggests one.
- **IPv4 only.** Only the IPv4 addresses of the listening server are offered to dial. An
  IPv6 address gets an error.
- **No private networks.** The option to run the tunnel over GRE or VXLAN is off for Paqet
  (see [Where it works](#where-it-works)).
- **Benchmark.** The [benchmark](panel.md) measures Paqet over the public IPv4 addresses
  in both directions when both servers run 2.8.0-beta.2 or newer. Choosing its result
  fills in the wizard.

## Where it works

- **IPv4 only.** Paqet does not run over IPv6 yet. The listening address and the address
  that is dialed must be IPv4 (or a name that resolves to IPv4).
- **Not inside GRE or VXLAN.** It would work technically, but there it gains nothing: a
  filter sees only the outer GRE or VXLAN packets, not the TCP inside them. Over a private
  network, `kcp` or `tcpmux` costs less.
- **A path that passes TCP without a handshake.** Some datacenters and providers run a
  stateful firewall in front of the servers that drops TCP segments of a connection it never
  saw open. Paqet cannot get through there. The tunnel then fails with `handshake timed out`,
  while `kcp` or `tcpmux` between the same servers work. Use another transport on such a
  route, or ask the provider to allow the port.

## Settings that apply

Paqet uses the same settings as `kcp`:

- **The profile** sets the KCP defaults. `gaming` turns on FEC (10 data and 3 parity
  packets), and the profile sets the socket buffers (1 MB, 4 MB with `ultraspeed`).
- **Multiplexing** is on by default and shares the link across many forwarded
  connections. `[tunnel.mux]` sets the number of connections, keepalive and windows. It can
  be turned off.
- **KCP tuning** (`[tunnel.kcp]`): timing mode, windows, MTU, FEC and datagrams. In the
  panel it is under **KCP settings** on the transport step, and it applies to Paqet only.
- **`[tuning]`**: socket buffer and DSCP mark, as for the other transports.

## Example reverse tunnel

Without the panel: generate one token with `kariz token` and use it on both sides. Replace
the example address and the forwarding target with yours.

```toml
# Entry (listener)
role = "entry"
mode = "reverse"

[tunnel]
transport = "paqet"
listen = "0.0.0.0:31080"
token = "CHANGE-ME-run-kariz-token"

[tunnel.kcp]
mtu = 1200

[[forward]]
listen = "0.0.0.0:18080"
target = "127.0.0.1:18081"
protocol = "tcp+udp"
```

```toml
# Exit (dialer)
role = "exit"
mode = "reverse"

[tunnel]
transport = "paqet"
remote = "192.0.2.21:31080"
token = "CHANGE-ME-run-kariz-token"

[tunnel.kcp]
mtu = 1200
```

`listen` may also name one address of the server (`192.0.2.21:31080`) to listen on that
one only.

## Firewall rules

Paqet segments carry data without a TCP handshake. Without rules, the kernel answers each
one with a TCP reset, and connection tracking marks them invalid. A host firewall that
accepts only established replies, such as ufw or firewalld, then drops them, and the tunnel
never connects.

Kariz therefore adds four `iptables` rules at the top of their chains. On the listening
side they are scoped to the tunnel's port (and to the listening address, when it is one
address). On the dialing side they are scoped to the dialer's address and the listener's
address and port, so every reconnect shares the same rules:

| Table and chain | Matches | Action |
| --- | --- | --- |
| `raw PREROUTING` | incoming tunnel segments | `CT --notrack` |
| `raw OUTPUT` | outgoing tunnel segments | `CT --notrack` |
| `filter INPUT` | incoming tunnel segments | `ACCEPT` |
| `filter OUTPUT` | outgoing TCP resets of the tunnel | `DROP` |

The rules carry the comment `kariz-paqet pid=<pid>`. They are removed when the tunnel stops.
If the process was killed, the next Paqet tunnel to start removes the rules it left behind.
List them with `sudo iptables -S | grep kariz-paqet` and
`sudo iptables -t raw -S | grep kariz-paqet`.

A firewall in front of the servers, such as a cloud provider's, is not on the host. Allow
the tunnel's TCP port there yourself, as for any other tunnel.

### Without iptables

If `iptables` is missing or fails, the tunnel still starts and the service log warns about
it. Then add equivalent rules yourself. Set these variables on both hosts with your own
IPv4 addresses and port. If a firewall service manages the rules, use its equivalent rules
and persistence mechanism.

```bash
listener=192.0.2.21
dialer=192.0.2.22
port=31080
action=-I
```

On the listener:

```bash
sudo iptables -w "$action" INPUT -s "$dialer" -d "$listener" -p tcp --dport "$port" -j ACCEPT
sudo iptables -w "$action" OUTPUT -s "$listener" -d "$dialer" -p tcp --sport "$port" --tcp-flags RST RST -j DROP
```

On the dialer:

```bash
sudo iptables -w "$action" INPUT -s "$listener" -d "$dialer" -p tcp --sport "$port" -j ACCEPT
sudo iptables -w "$action" OUTPUT -s "$dialer" -d "$listener" -p tcp --dport "$port" --tcp-flags RST RST -j DROP
```

If the OUTPUT policy is restrictive, allow the matching outbound TCP data after the RST drop
rule. Run the same commands with `action=-D` to remove these rules.

## Tuning

The kernel filters other TCP ports before userspace, and on the dialing side other source
addresses too. Kariz reuses packet buffers and authenticates incoming packets before it
starts a new KCP conversation.

Each flow keeps its own TCP sequence and acknowledgment numbers, as a real connection would.
A stateful firewall on the path then sees each mux connection stay inside its own window.
The raw socket buffers are set to the `[tuning]` socket buffer size even above
`net.core.rmem_max` and `wmem_max`, so a burst is not lost to a small default.

The default KCP MTU is 1350. For a path with a smaller MTU, lower it on both endpoints. A
value of 1200 is a starting point. Subtract at least 70 bytes from the IPv4 path MTU, or 83
with FEC. Kariz caps the setting at 1430, or 1417 with FEC.

Start with the defaults. On a quiet link, the `normal` timing preset flushes less often than
`fast2`. Smaller windows reduce queued packets at the cost of throughput over long or lossy
routes. FEC adds parity traffic and CPU work, so enable it when packet loss warrants it.

## Troubleshooting

| What you see | Why | What to do |
| --- | --- | --- |
| `handshake timed out`, while `kcp` or `tcpmux` work between the same servers | something in front of a server drops TCP without a handshake | try another transport on that route, or ask the provider to allow the port |
| `paqet needs root or CAP_NET_RAW` | the tunnel does not run as the Kariz service | run it with the `kariz@` service, or as root |
| `paqet could not add a firewall rule` in the log | `iptables` is missing or failed | add the rules [by hand](#without-iptables) |
| `paqet supports IPv4 only` | an IPv6 address to listen on or to dial | use the IPv4 address |
| `old_agent_paqet` in the panel | a server runs a Kariz older than 2.8.0-beta.1 | update it (Settings, Updates) |

To see the packets, run `sudo tcpdump -ni any 'tcp port 31080'` on both servers while the
tunnel connects (with your port). Segments that leave one server and never reach the other
are dropped on the path. Ordinary TCP port checks cannot test Paqet, because nothing on the
port answers a normal connection.

## Versions

- **2.8.0-beta.1:** Paqet. The listener needs one concrete IPv4 address.
- **2.8.0-beta.2:** the firewall rules are added automatically, and the benchmark measures
  Paqet.
- **2.8.1:** the listener can be on every IPv4 address (`0.0.0.0`). For an older
  listening server the panel uses that server's own IPv4 address instead.

Paqet's packet format and token-derived keys are specific to Kariz. It is not
wire-compatible with the upstream Go project of the same name.
