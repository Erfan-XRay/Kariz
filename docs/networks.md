# Private networks (GRE)

The panel can join your servers into a **private network**: each pair gets a GRE link with
private addresses (`10.77.0.1`, `10.77.0.2`, ...) that the panel hands out so that **no two are
ever the same**, however many servers there are. A direct tunnel can then listen on and dial
those private addresses, so it never uses the servers' public ones.

## What GRE is, and what it is not

GRE is a feature of the Linux kernel (`ip tunnel ... mode gre`), not a Kariz transport. Kariz's
own transports are unchanged; a tunnel simply uses the private addresses. The tunnel's records
are still encrypted by Kariz (its token and cipher), so what crosses the wire is encrypted even
though GRE itself is not.

GRE is plain IP protocol 47. It is easy to recognise, and some providers and NATs block it or
do not forward it. It suits two servers whose public addresses reach each other; **it is not for
hiding from filtering**. The panel tests that packets pass before it keeps a link, and says so
if they do not.

## Use it

1. **Servers need an address.** On the *Networks* page, under *Server addresses*, set the
   address each server is reached at by the others (an IPv4 address; the panel's own server is
   filled in from the address you opened the panel by). GRE needs both ends.
2. **Make a network:** *New network*, a name and a pool, for example `10.77.0.0/16`. The pool
   must be inside `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` or `100.64.0.0/10`, be `/24` or
   larger, not overlap another network, and not overlap a network any connected server already
   routes (Docker's `172.17.0.0/16`, a cloud's private network...). The panel says which server
   and which route when it does.
3. **Add links:** *Add links*, pick the servers. A **mesh** joins every pair (three servers, three
   links; N servers, N(N-1)/2); **hub and spoke** joins one server to each of the others (N-1
   links), for many servers. For each link the panel checks that both servers can make GRE,
   takes the first free `/30` (two usable addresses and nothing else), makes the interface on
   both, and pings across it. If the ping fails the link is removed again and you are told why.
4. **Use it in a tunnel:** in the wizard, a *direct* tunnel has *Use a private GRE network
   between these servers*. The tunnel then listens on the exit's end of the link and the entry
   dials it. If the two servers have no link yet in that network, the panel makes one first
   (and removes it again if the tunnel cannot be made).

## The rules the panel keeps

- No address is given twice, ever, until its link is deleted. The database refuses a repeated
  subnet or address whatever the code does, so even two requests at the same moment cannot
  both win.
- Two networks never overlap, and a pool never overlaps what a connected server routes. A
  link's `/30` also skips any block that overlaps a route of either of its two servers.
- Two links between the same pair get different GRE keys (the number that tells the kernel two
  GRE tunnels between the same two addresses apart), the smallest one not yet used.
- A server that changes its public address keeps its private ones: set the new address on the
  page and the panel sends every link of that server again.
- A link a tunnel still uses (its listen or dial address is one of the link's) cannot be
  deleted. Deleting a tunnel keeps its link (the panel does not guess you are done with it).
- Removing a server removes its links, and the servers on their other ends are told.

## On the servers

The agent makes **one `ip` invocation per step**, with fixed arguments and values it has
checked: `ip tunnel add kz-XXXXX mode gre local A remote B key K ttl 64`, `ip addr add
ADDR/30 dev kz-XXXXX`, `ip link set kz-XXXXX mtu 1476 up`. It only ever touches interfaces named
`kz-` and up to eight small letters or digits; it takes a link as data, never a command.
The links are kept in `net.toml` beside `agent.toml` (`/etc/kariz-panel/net.toml`), and the
panel sends the whole list again whenever an agent connects, so a reboot, or a change made
while a server was away, is put right by itself: links that are missing or changed are made,
and links that are no longer wanted are removed.

The MTU is 1476 (GRE adds 24 bytes to a 1500-byte path).

- The panel is where the links are made, shown and removed. On a server, `ip -br addr show` lists
  its `kz-` interfaces and `ip -s link show kz-XXXXX` shows one with its counters.
- **Servers that cannot do it:** GRE needs root and the `ip_gre` module. Some containers and
  VPS types (OpenVZ) do not allow it: the panel says `cannot make GRE interfaces` for that
  server and does not try.
- **A firewall in between** that drops IP protocol 47 makes the test fail ("packets do not pass
  ... over GRE"). Open it on the server that receives, for example
  `iptables -I INPUT -p gre -s PEER -j ACCEPT`, or use another mode. The panel never changes a
  firewall itself.

## Testing

`tests/gre_ns.sh` (run by CI as root) builds two network namespaces as two servers, makes GRE
links between them with the agent's code, pings across, makes a second link between the same
pair, checks that a filtered GRE path fails the test, runs a Kariz tunnel over the private
addresses end to end, and removes the links.
