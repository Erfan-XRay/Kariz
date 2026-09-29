# Phase 13 plan: private networks with GRE

Target release: **v0.10.0**. Scope: in the tunnel wizard, a direct-mode tunnel can run over a
**private GRE network** the panel sets up between the servers, with private addresses it hands
out so that no two are ever the same, however many servers are joined.

## 1. Why, and what it is not

- **Why.** A user with two or three servers often wants them to see each other as one private
  network (`10.77.x.x`), so that tunnels, panels, databases and monitoring talk over private
  addresses instead of each server's public one, and so that adding a third server does not
  mean planning addresses by hand.
- **GRE is not a Kariz transport.** It is a feature of the Linux kernel (`ip tunnel ... mode
  gre`) that the agent sets up on each server. Kariz's own transports (tcp, tcpmux, ws, wss,
  quic, kcp) are unchanged; a Kariz tunnel simply *listens on and dials the private
  addresses*. The tunnel's records stay encrypted by Kariz (its token and AEAD), so the
  traffic on the wire is encrypted even though GRE itself is not.
- **What to tell the user honestly.** GRE is plain IP protocol 47: easy to recognise and
  often blocked or not forwarded by NATs and some providers. It suits paths that are open
  between two public addresses; it is not for hiding from DPI. The wizard says so, and the
  panel tests the path before it commits (section 4).

## 2. In the wizard

Step 3 (connection) gains, **only when the mode is direct** (both servers reach each other's
public address; reverse mode exists for the case where they cannot):

```text
[ ] Use a private GRE network between these servers
    Network:  [ main (10.77.0.0/16) v ]   or  [ Make a new network... ]
    entry  tehran-1    10.77.0.1   <->   frankfurt  10.77.0.2
    The tunnel will listen on 10.77.0.2:3080 and dial it from 10.77.0.1.
```

The addresses shown are the ones the panel reserved; the tunnel's listen and remote
addresses are filled in from them. Unchecked, nothing changes from today. The review step
lists what will be made on each server (the GRE interface, its address, the route).

## 3. Private networks and addresses (the part that must not repeat)

A **network** is a pool the panel owns: a name and a CIDR. Every GRE link between two
servers gets its own **/30** from the pool (a /31 where both kernels take it), so a link is
two usable addresses and nothing else, and links never share an address. With three servers
A, B and C in a full mesh there are three links (A-B, A-C, B-C), six addresses, none equal;
adding D adds three more links, and so on.

Rules the panel enforces, in one SQLite transaction (a `UNIQUE` on every address, so even two
requests at once cannot both win):

1. **No address is given twice** in the panel, ever, until its link is deleted.
2. **The pool is a private range** (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 or
   100.64.0.0/10), no smaller than /24, and two networks may not overlap.
3. **Nothing on the servers collides.** The agent's `health` report now also lists the
   addresses and routes each server already has (Docker's `172.17.0.0/16`, a VPN, a cloud's
   `10.x` private network). A link's addresses, and the pool itself, are refused when they
   overlap one on either server; the wizard says which server and which route.
4. **Two links between the same pair of servers** (a second tunnel) each get their own GRE
   *key* (a unique 32-bit number), which is what tells the kernel two GRE tunnels between the
   same public addresses apart.
5. **A server that changes its public address** keeps its private ones: the link is defined
   by the two servers, and only `local` and `remote` of the interface change.
6. **Deleting** a tunnel releases nothing until its link is deleted too (the panel asks).

Hub and spoke is offered besides the full mesh (`tehran-1` as the hub: N-1 links instead of
N(N-1)/2), for the case of many servers.

## 4. What the agent does (still a fixed list, still no shell)

Two new requests, `net_up` and `net_down`. They carry data, never a command:

```text
net_up   { name: "kz-a1b2", local: "203.0.113.5", remote: "198.51.100.7",
           key: 4242, address: "10.77.0.1", peer: "10.77.0.2", prefix: 30, mtu: 1476 }
net_down { name: "kz-a1b2" }
```

- Every field is parsed and checked (the name matches `kz-[a-z0-9]{1,8}`, the addresses are
  IP addresses in the network's pool, the key and the MTU are in range) before anything runs.
- The agent runs fixed `ip` invocations with the checked values as arguments (no shell, no
  string built from what the panel sent): create the GRE interface, address it, set the MTU
  (1476 by default: GRE adds 24 bytes), bring it up. It only ever touches interfaces whose
  names start with `kz-`.
- **It survives a reboot:** the agent keeps the desired interfaces in
  `/etc/kariz-panel/net.toml` and re-creates them at start; the panel also re-sends them on
  every connection, so the two agree.
- **A path test before the tunnel is made:** the two agents bring the link up and ping across
  it. If GRE does not pass (blocked protocol, a NAT), the wizard says so, removes what it made,
  and suggests the other modes.
- **Firewall:** the agent reports whether IP protocol 47 seems filtered, and the wizard
  shows the one rule to add when it is (`iptables -I INPUT -p gre -s <peer> -j ACCEPT`), but
  never changes the firewall itself.
- The agent needs root (it already runs as root) and the `ip_gre` kernel module (loaded on
  demand). Containers and some VPS types (OpenVZ) cannot make GRE: the agent reports
  `gre: unavailable` and the checkbox is disabled for that server, with the reason.

## 5. Also in this phase

- A **Networks** page: each network, its pool and how full it is, its links (the two
  servers, addresses, key, state, round-trip time from the ping), and warnings.
- The map draws a private link as a thin line under the horizon between the two wells.
- `kariz-manager net` on a server: `list` and `status` of its `kz-` interfaces (read only).

## 6. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **13.0** Plan | This document. | |
| **13.1** Addresses | Networks and links in SQLite, the allocator and its rules (section 3), the report of each server's addresses and routes; unit tests for every rule, including two requests at the same moment. | No test can make a duplicate or overlapping address. |
| **13.2** Agent | `net_up` / `net_down`, validation, `net.toml`, the path test, the firewall hint. | Tests on the parsers and the validation; a CI job that builds two network namespaces with GRE between them and pings across. |
| **13.3** Wizard and pages | The checkbox and its step, the Networks page, the map line, the review. | Both languages and themes; a tunnel made over GRE between two namespaces in CI. |
| **13.4** Release | Docs (`docs/networks.md`), CHANGELOG, `0.10.0`. | CI green; release. |
