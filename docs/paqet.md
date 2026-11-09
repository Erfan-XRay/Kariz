# Paqet over raw TCP

Paqet is a tunnel transport for Kariz on Linux and IPv4. It carries authenticated,
encrypted KCP packets in raw TCP segments. Kariz builds the TCP headers and uses Linux
routing to send the packets. Both endpoints need Kariz 2.8.0-beta.1 or later and
`CAP_NET_RAW`. The Kariz tunnel service already grants that capability.

Choose a dedicated TCP port and the listener's local IPv4 address. The panel fills in
the address that the listening server's route to the internet uses. In reverse mode the
entry listens and the exit dials. In direct mode the exit listens and the entry dials.
Kariz adds the host firewall rules Paqet needs on both sides itself (see below).

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

Paqet segments carry data without a TCP handshake. Without rules, the kernel answers
each one with a TCP reset, and connection tracking marks them invalid. A host firewall
that accepts only established replies, such as ufw or firewalld, then drops them, and
the tunnel never connects.

Kariz therefore adds four `iptables` rules at the top of their chains. On the
listening side they are scoped to the listener's address and port. On the dialing side
they are scoped to the dialer's address and the listener's address and port, so every
reconnect shares the same rules:

| Table and chain | Matches | Action |
| --- | --- | --- |
| `raw PREROUTING` | incoming tunnel segments | `CT --notrack` |
| `raw OUTPUT` | outgoing tunnel segments | `CT --notrack` |
| `filter INPUT` | incoming tunnel segments | `ACCEPT` |
| `filter OUTPUT` | outgoing TCP resets of the tunnel | `DROP` |

The rules carry the comment `kariz-paqet pid=<pid>`. They are removed when the tunnel
stops. If the process was killed, the next Paqet tunnel to start removes the rules it
left behind. List them with `sudo iptables -S | grep kariz-paqet` and
`sudo iptables -t raw -S | grep kariz-paqet`.

A firewall in front of the servers, such as a cloud provider's, is not on the host.
Allow the tunnel's TCP port there yourself.

### Without iptables

If `iptables` is missing or fails, the tunnel still starts and the service log warns
about it. Then add equivalent rules yourself. Set these variables on both hosts with
your own IPv4 addresses and port. If a firewall service manages the rules, use its
equivalent rules and persistence mechanism.

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
drop rule. Run the same commands with `action=-D` to remove these rules.

A stateful firewall or NAT between hosts may drop synthetic TCP. Use a routable IPv4
path that passes it, and confirm with a real forwarding request. Ordinary TCP port
probes cannot validate Paqet. Check the service log or
`tcpdump -ni any 'tcp port 31080'` on each host if the tunnel fails to connect.

## Tuning

Paqet uses the existing [KCP settings](configuration.md#tunnelkcp-kcp) for timing,
windows, MTU, FEC and datagrams. Multiplexing is enabled by default and shares the
link across many forwarded connections. The kernel filters other TCP ports before
userspace, and on the dialing side other source addresses too. Kariz reuses packet
buffers and authenticates incoming packets before allocating a new KCP conversation.

Each flow keeps its own TCP sequence and acknowledgment numbers, as a real connection
would. A stateful firewall on the path then sees each mux connection stay inside its
own window. The raw socket buffers are set to the `[tuning]` socket buffer size even
above `net.core.rmem_max` and `wmem_max`, so a burst is not lost to a small default.

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
