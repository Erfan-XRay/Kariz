# Paqet over raw TCP

Paqet is a tunnel transport for Kariz on Linux and IPv4. It carries authenticated,
encrypted KCP packets in raw TCP segments. Kariz builds the TCP headers and uses Linux
routing to send the packets. Both endpoints need Kariz 2.8.0-beta.1 or later and
`CAP_NET_RAW`. The Kariz tunnel service already grants that capability.

Choose a dedicated TCP port and the listener's local IPv4 address. In reverse mode the
entry listens and the exit dials. In direct mode the exit listens and the entry dials.
Prepare the firewall on both hosts before creating the tunnel in the panel.

## Example reverse tunnel

Generate one token with `kariz token` and use it on both sides. Replace the example
addresses and the forwarding target with yours.

```toml
# Entry (listener)
role = "entry"
mode = "reverse"

[tunnel]
transport = "paqet"
listen = "192.0.2.21:31080"
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

## Firewall on both hosts

Paqet segments carry data without a TCP handshake. The kernel can send TCP resets
for these packets. Allow Paqet traffic through the host firewall and drop those
outgoing resets using rules scoped to the peer addresses and tunnel port.

Set these variables on both hosts and substitute your own IPv4 addresses and port.
If the rules are managed by a firewall service, use its equivalent rules and
persistence mechanism. The examples assume IPv4 routing between the hosts.

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

If the OUTPUT policy is restrictive, allow the matching outbound TCP data after the RST
drop rule. Apply matching permissions at any provider firewall. Run the same commands
with `action=-D` to remove these rules.

A stateful firewall or NAT between hosts may drop synthetic TCP. Use a routable IPv4
path that passes it, and confirm with a real forwarding request. Ordinary TCP port
probes cannot validate Paqet. Check the service log or
`tcpdump -ni any 'tcp port 31080'` on each host if the tunnel fails to connect.

## Tuning

Paqet uses the existing [KCP settings](configuration.md#tunnelkcp-kcp) for timing,
windows, MTU, FEC and datagrams. Multiplexing is enabled by default and shares the
link across many forwarded connections. The kernel filters other TCP ports before
userspace. Kariz reuses packet buffers and authenticates incoming packets before
allocating a new KCP conversation.

The default KCP MTU is 1350. For a path with a smaller MTU, lower it on both endpoints.
A value of 1200 is a starting point. Subtract at least 70 bytes from the IPv4 path MTU,
or 83 with FEC. Kariz caps the setting at 1430, or 1417 with FEC.

Start with the defaults. On a quiet link, the `normal` timing preset flushes less often
than `fast2`. Smaller windows reduce queued packets at the cost of throughput over
long or lossy routes. FEC adds parity traffic and CPU work, so enable it when packet
loss warrants it. Paqet uses the existing `[tunnel.mux]` and `[tuning]` settings for
connection count, keepalive, socket buffer and DSCP.

Paqet is selected explicitly for a tunnel. Its packet format and token-derived keys
are specific to Kariz.
