# Private networks (GRE and VXLAN)

The panel can join your servers into a **private network**: each pair gets a GRE or VXLAN link with
private addresses (`10.77.0.1`, `10.77.0.2`, ...) that the panel hands out so that **no two are
ever the same**, however many servers there are. A tunnel, direct or reverse, can then listen on
and dial those private addresses, so it never uses the servers' public ones.

## What GRE is, and what it is not

GRE is a feature of the Linux kernel (`ip tunnel ... mode gre`), not a Kariz transport. Kariz's
own transports are unchanged; a tunnel simply uses the private addresses. The tunnel's records
are still encrypted by Kariz (its token and cipher), so what crosses the wire is encrypted even
though GRE itself is not.

GRE is plain IP protocol 47. It is easy to recognise, and some providers and NATs block it or
do not forward it. It suits two servers whose public addresses reach each other; **it is not for
hiding from filtering**. The panel tests that packets pass before it keeps a link, and says so
if they do not.

## GRE or VXLAN

A network is one or the other, chosen when it is made, and all its links are of that kind.

| | GRE | VXLAN |
|---|---|---|
| On the wire | IP protocol 47 | UDP datagrams to one port (4789 unless you pick another) |
| Added per packet | 24 bytes (MTU 1476) | 50 bytes (MTU 1450) |
| Passes where | protocol 47 is let through | UDP to that port is let through |
| Kernel module | `ip_gre` | `vxlan` |
| Needs on both servers | any version | Kariz 2.6 or newer |

Pick **VXLAN** when GRE does not get through between your two servers (a provider or a NAT that
drops protocol 47) but UDP does; otherwise GRE is lighter. VXLAN is no harder to recognise than
GRE: it is plain UDP to a fixed port, and the frames inside are not encrypted by it (the Kariz
tunnels over it still are). If one port is blocked, make a second VXLAN network on another port:
every network has its own.

Each VXLAN link has one peer: its interface sends everything to the other server's public
address, so nothing is flooded and no multicast is needed. Its **VNI** (the number that tells
VXLAN links apart) is chosen by the panel so that no two VXLAN links of a server share one: the
kernel takes a VNI once per port on a server, whoever the other end is.

## Use it

1. **Servers need an address.** On the *Networks* page, under *Server addresses*, set the
   address each server is reached at by the others (an IPv4 address; the panel's own server is
   filled in from the address you opened the panel by). GRE needs both ends.
2. **Make a network:** *New network*, the link type (GRE, or VXLAN and its UDP port), a name and
   a pool, for example `10.77.0.0/16`. The pool
   must be inside `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` or `100.64.0.0/10`, be `/24` or
   larger, not overlap another network, and not overlap a network any connected server already
   routes (Docker's `172.17.0.0/16`, a cloud's private network...). The panel says which server
   and which route when it does.
3. **Add links:** *Add links*, pick the servers. A **mesh** joins every pair (three servers, three
   links; N servers, N(N-1)/2); **hub and spoke** joins one server to each of the others (N-1
   links), for many servers. For each link the panel checks that both servers can make the link
   (GRE or VXLAN, and for VXLAN that both run 2.6 or newer),
   takes the first free `/30` (two usable addresses and nothing else), makes the interface on
   both, and pings across it. If the ping fails the link is removed again and you are told why.
4. **Use it in a tunnel:** in the wizard, a tunnel (reverse or direct) has *Use a private network
   (GRE or VXLAN) between these servers*. The tunnel then listens on the private address of the server
   that accepts its connections (the entry when reverse, the exit when direct) and the other server
   dials it. If the two servers have no link yet in that network, the panel makes one first
   (and removes it again if the tunnel cannot be made).
5. **Change it later:** the tunnel's edit page shows the network it runs over. Tick or untick it,
   or pick another network, and save: both sides are changed together and put back if they do not
   connect. Unticked, the tunnel goes back to the public addresses (you give the address to dial);
   the link stays for other tunnels.

## The agent's link over a network

A server that already has a GRE link to the panel's own server can send its **agent** through it:
the agent dials the panel at the panel's address on the link (such as `10.77.0.1`) instead of a
public one. Nothing is different for the agent otherwise: all four transports work over it.

- In *Servers*, *Edit* (or *Reconnect*, for a server that is offline) offers that address, labelled
  *GRE · network*, beside the public ones, and picks it by itself for a server whose agent already
  connects that way. Make the code and run it on the server as usual; the server keeps its name,
  tunnels and links. A server whose agent comes in through a link shows *via GRE* on its card.
- **A new server can join over GRE alone** ([below](#add-a-server-over-gre)).
- The link comes back by itself after a reboot: the agent makes the links it had before it dials
  the panel, from the settings it saved. The panel's own server does the same, and the agents port
  listens on every address, so open it in the firewall for the link's interface too.
- A link that an agent reaches the panel through **cannot be deleted** (like a link a tunnel uses):
  the panel would lose the only way it has to ask that server anything. Switch the agent back to a
  public address first (*Servers*, *Edit*). If it happens anyway (the server's public address
  changed, GRE blocked on the path), the server shows offline: *Reconnect* it with a public address.

## Add a server over GRE

For a server that can reach the panel's server **only over GRE** (the agents port is filtered
between them, GRE is not): *Servers*, *Add server*, **Over a GRE link**, and type the new
server's public IPv4 address. That is all there is to fill in.

**Over a VXLAN link** beside it does the same with a VXLAN link, for when GRE does not pass but
UDP does: the link's addresses come from the first VXLAN network, or from a new one the panel
makes (`kariz-vxlan`, UDP 4789, on the first free pool), and then only UDP to that network's
port has to pass, both ways. The new server needs Kariz 2.6 or newer, which the command installs.
*The panel connects to it* and a server's *Edit* (*Across a private link*) have the same choice,
GRE or VXLAN, for a new link.

1. The panel lists the server (it shows *waiting for its agent*), gives the link between the two
   a `/30` of a network (the first one there is; with none, it makes `kariz`, `10.77.0.0/16` or
   the next free pool), and makes **its own end of the GRE link now**, from its public address
   to the one you typed.
2. You run the command it shows on the new server, as root (it installs Kariz first if needed).
   The join code carries the server's end of the link: the agent makes it, then dials the panel's
   private address across it (such as `10.77.0.2`). The server turns *online*, *via GRE*.
3. Only GRE (IP protocol 47) has to pass between the two public addresses, or, when the link's
   network is a VXLAN one, UDP to its port. The link is saved on both servers and comes back
   after a reboot, before the agent dials.

The panel must know its own server's public IPv4 address (*Networks*, *Server addresses*); it is
shown in the dialog. A code works once, for 10 minutes: if it runs out, *New command* on the
waiting server's card makes another (it carries the link too). *Remove* on that card takes the
server, its link and the panel's end of it away again. An address another server has already is
refused: bring that server back with *Reconnect* instead.

## The rules the panel keeps

- No address is given twice, ever, until its link is deleted. The database refuses a repeated
  subnet or address whatever the code does, so even two requests at the same moment cannot
  both win.
- Two networks never overlap, and a pool never overlaps what a connected server routes. A
  link's `/30` also skips any block that overlaps a route of either of its two servers.
- Two links between the same pair get different GRE keys (the number that tells the kernel two
  GRE tunnels between the same two addresses apart), the smallest one not yet used. A VXLAN link
  takes the smallest VNI that no other link of either of its two servers has.
- A server that changes its public address keeps its private ones: set the new address on the
  page and the panel sends every link of that server again.
- A link a tunnel still uses (its listen or dial address is one of the link's), or that a
  server's agent reaches the panel through, cannot be deleted. Deleting a tunnel keeps its link (the panel does not guess you are done with it).
- Removing a server removes its links, and the servers on their other ends are told.
- An update does not break a network. The panel remembers the address each server was last known
  by, so after it restarts it still sends every server its whole list of links, and it never sends
  a partial one (an agent removes what is not in the list). An agent that only restarts leaves
  GRE interfaces the kernel already has alone, so the tunnels over them keep their connections.

## On the servers

The agent makes **one `ip` invocation per step**, with fixed arguments and values it has
checked: `ip tunnel add kz-XXXXX mode gre local A remote B key K ttl 64`, `ip addr add
ADDR/30 dev kz-XXXXX`, `ip link set kz-XXXXX mtu 1476 up`; for VXLAN the first is
`ip link add kz-XXXXX type vxlan id VNI local A remote B dstport PORT ttl 64` and the MTU is
1450. It only ever touches interfaces named `kz-` and up to eight small letters or digits; it
takes a link as data, never a command.
The links are kept in `net.toml` beside `agent.toml` (`/etc/kariz-panel/net.toml`), and the
panel sends the whole list again whenever an agent connects, so a reboot, or a change made
while a server was away, is put right by itself: links that are missing or changed are made,
and links that are no longer wanted are removed.

The MTU is 1476 for GRE (it adds 24 bytes to a 1500-byte path) and 1450 for VXLAN (50 bytes).

- The panel is where the links are made, shown and removed. On a server, `ip -br addr show` lists
  its `kz-` interfaces and `ip -s link show kz-XXXXX` shows one with its counters.
- **Servers that cannot do it:** GRE needs root and the `ip_gre` module, VXLAN the `vxlan`
  module. Some containers and VPS types (OpenVZ) do not allow them: the panel says `cannot make
  GRE interfaces` (or VXLAN) for that server and does not try. An agent older than 2.6 cannot
  make VXLAN at all: the panel names it and asks for an update.
- **A firewall in between** that drops IP protocol 47 makes the test fail ("packets do not pass
  ... over GRE"). Open it on the server that receives, for example
  `iptables -I INPUT -p gre -s PEER -j ACCEPT`, or use another mode. For VXLAN it is UDP to the
  network's port on **both** servers ("packets do not pass ... over VXLAN: ... UDP port 4789"):
  `iptables -I INPUT -p udp --dport 4789 -s PEER -j ACCEPT`. The panel never changes a
  firewall itself.

## Testing

`tests/gre_ns.sh` (run by CI as root) builds two network namespaces as two servers, makes GRE
links between them with the agent's code, pings across, makes a second link between the same
pair, checks that a filtered GRE path fails the test, runs a Kariz tunnel over the private
addresses end to end, and removes the links. Then it does the same with VXLAN links: one on port
4789, a second between the same pair (which the kernel refuses with the same VNI, and takes with
the next), one on port 8472 with VNI 1 again, a filtered port, and the saved list.
