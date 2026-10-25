//! Private networks as operations: making the links of a network (with their interfaces on
//! both servers and a path test), removing them, and keeping each server's list of links
//! in step with the panel's.
//!
//! Like tunnels (`pair.rs`) these run in the background as steps the browser follows, and
//! a step that fails undoes what the earlier ones did.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use crate::hub::Hub;
use crate::networks::{mesh, Link};
use crate::wire::{Ack, LinkKind, NetSpec, NetStatus, PingReply, Request};

/// The first version whose agent makes VXLAN links. An older one refuses the spec (it does
/// not know its fields), so the panel says so before it tries.
pub const VXLAN_SINCE: (u32, u32, u32) = (2, 6, 0);

/// How long a server is waited for while the interfaces of deleted links are removed from
/// it. Deleting never waits on a server for longer: one that does not answer is asked again.
pub const DOWN_WAIT: Duration = Duration::from_secs(5);

/// Whether `text` is an address a server can be reached at: IPv4, unicast.
pub fn valid_addr(text: &str) -> bool {
    text.parse::<Ipv4Addr>().is_ok_and(|a| {
        !(a.is_unspecified() || a.is_loopback() || a.is_multicast() || a.is_broadcast())
    })
}

/// One end of a link, as the server that owns it is told: `mine` is the address of this
/// end, `theirs` the other's.
pub(crate) fn spec_for(link: &Link, server: &str, local: &str, remote: &str) -> NetSpec {
    let first = link.a == server;
    NetSpec {
        name: link.ifname.clone(),
        local: local.to_owned(),
        remote: remote.to_owned(),
        key: link.gre_key,
        address: if first { &link.addr_a } else { &link.addr_b }.clone(),
        peer: if first { &link.addr_b } else { &link.addr_a }.clone(),
        prefix: 30,
        mtu: crate::net::max_mtu(link.kind),
        kind: link.kind,
        port: link.port,
    }
}

/// The address a tunnel over `link` listens on and dials: the end of the link on the server
/// that accepts the tunnel's connections (the exit in direct mode, the entry in reverse
/// mode), with the port of the wizard's listen address.
pub fn tunnel_endpoint(link: &Link, acceptor: &str, listen: &str) -> String {
    let addr = if link.a == acceptor {
        &link.addr_a
    } else {
        &link.addr_b
    };
    format!("{addr}:{}", listen.rsplit(':').next().unwrap_or("0"))
}

impl Hub {
    /// The specs of every link of `server`. An agent removes the links that are not in the
    /// list it is sent, so the list is never partial: if the public address of either end of
    /// a link is not known, no list is made (and the agent keeps the links it has).
    pub fn net_specs(&self, server: &str) -> Result<Vec<NetSpec>> {
        let mut out = Vec::new();
        for link in self.networks.links(None)? {
            if link.a != server && link.b != server {
                continue;
            }
            let other = if link.a == server { &link.b } else { &link.a };
            let (Some(local), Some(remote)) = (self.addr_of(server), self.addr_of(other)) else {
                bail!("no_address:{}", link.ifname);
            };
            out.push(spec_for(&link, server, &local, &remote));
        }
        Ok(out)
    }

    /// The servers at the other end of `server`'s links, once each.
    pub fn net_peers(&self, server: &str) -> Vec<String> {
        let mut peers: Vec<String> = self
            .networks
            .links(None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|l| {
                if l.a == server {
                    Some(l.b)
                } else if l.b == server {
                    Some(l.a)
                } else {
                    None
                }
            })
            .collect();
        peers.sort();
        peers.dedup();
        peers
    }

    /// Tells the servers at the other ends of `server`'s links their lists again, in the
    /// background (the ones that are connected; the others are told when they connect).
    pub fn net_sync_peers(self: &Arc<Self>, server: &str) {
        for peer in self.net_peers(server) {
            if !self.is_online(&peer) {
                continue;
            }
            let hub = self.clone();
            tokio::spawn(async move {
                if let Err(e) = hub.net_sync(&peer).await {
                    tracing::warn!(server = %peer, error = %e, "the private network links could not be synced");
                }
            });
        }
    }

    /// Removes from `server` the interfaces of its deleted links ([`Networks::pending_downs`]),
    /// one `NetDown` each: they need no address, so a link whose other end is unknown does
    /// not hold them up. Each one the server removed is forgotten; the others stay for the
    /// next try. Gives up after [`DOWN_WAIT`] (a server that does not answer is tried again
    /// later). Ok when none is left.
    ///
    /// [`Networks::pending_downs`]: crate::networks::Networks::pending_downs
    pub async fn net_flush(&self, server: &str) -> Result<()> {
        let names = self.networks.pending_downs(server)?;
        if names.is_empty() {
            return Ok(());
        }
        if !self.is_online(server) {
            bail!("offline");
        }
        let work = async {
            for name in names {
                let ack: Ack = self
                    .ask_as_within(server, &Request::NetDown { name: name.clone() }, DOWN_WAIT)
                    .await?;
                if !ack.ok {
                    bail!(ack.error.unwrap_or_else(|| "failed".into()));
                }
                self.networks.down_done(server, &name)?;
            }
            Ok(())
        };
        tokio::time::timeout(DOWN_WAIT, work)
            .await
            .map_err(|_| anyhow!("no answer"))?
    }

    /// Tries again, in the background, on every connected server that still has interfaces
    /// of deleted links.
    pub fn net_flush_all(self: &Arc<Self>) {
        for server in self.networks.servers_with_downs().unwrap_or_default() {
            if !self.is_online(&server) {
                continue;
            }
            let hub = self.clone();
            tokio::spawn(async move {
                if let Err(e) = hub.net_flush(&server).await {
                    tracing::info!(server = %server, error = %format!("{e:#}"), "deleted links' interfaces are still there");
                }
            });
        }
    }

    /// Tells a server its whole list of links (an agent that just connected, a change).
    pub async fn net_sync(&self, server: &str) -> Result<()> {
        let links = self.net_specs(server)?;
        let ack: Ack = self.ask_as(server, &Request::NetSync { links }).await?;
        if ack.ok {
            Ok(())
        } else {
            bail!(ack.error.unwrap_or_else(|| "failed".into()))
        }
    }
}

async fn expect_ack(hub: &Hub, server: &str, request: Request) -> Result<(), String> {
    let ack: Ack = hub
        .ask_as(server, &request)
        .await
        .map_err(|e| format!("{e:#}"))?;
    if ack.ok {
        Ok(())
    } else {
        Err(ack.error.unwrap_or_else(|| "failed".into()))
    }
}

/// Removes a link: its row at once, then its interface on both servers, asked at the same
/// time. Returns the servers that have not removed it yet (offline, no answer within
/// [`DOWN_WAIT`], or an error): the panel asks them again when they connect and every few
/// minutes, so the interface goes in the end whatever happened here.
pub async fn tear_down(hub: &Hub, link: &Link) -> Result<Vec<String>> {
    hub.networks.delete_link(&link.id)?;
    let (a, b) = tokio::join!(hub.net_flush(&link.a), hub.net_flush(&link.b));
    let mut left = Vec::new();
    for (server, done) in [(&link.a, a), (&link.b, b)] {
        if let Err(e) = done {
            tracing::info!(server = %server, link = %link.ifname, error = %format!("{e:#}"), "a deleted link's interface is removed later");
            left.push(server.clone());
        }
    }
    Ok(left)
}

/// Makes a link's interface on both servers and tests the path across it.
async fn bring_up(hub: &Hub, link: &Link) -> Result<(), String> {
    for server in [&link.a, &link.b] {
        let other = if &link.a == server { &link.b } else { &link.a };
        let (Some(local), Some(remote)) = (hub.addr_of(server), hub.addr_of(other)) else {
            return Err(format!("no_address:{server}"));
        };
        expect_ack(
            hub,
            server,
            Request::NetUp {
                net: spec_for(link, server, &local, &remote),
            },
        )
        .await?;
    }
    let reply: PingReply = hub
        .ask_as(
            &link.a,
            &Request::NetPing {
                name: link.ifname.clone(),
            },
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
    if reply.ok {
        Ok(())
    } else {
        Err(match link.kind {
            LinkKind::Gre => "gre_blocked".to_owned(),
            LinkKind::Vxlan => format!("vxlan_blocked:{}", link.port),
        })
    }
}

/// Whether `server` can make links of `kind`: connected, with an agent new enough, and a
/// kernel that allows it. Errors: `offline:ID`, `old_agent_vxlan:ID:VERSION`, `no_gre:ID`,
/// `no_vxlan:ID`.
pub async fn check_kind(hub: &Hub, server: &str, kind: LinkKind) -> Result<(), String> {
    if kind == LinkKind::Vxlan {
        let version = hub
            .snapshot()
            .ok()
            .and_then(|all| all.into_iter().find(|s| s.id == server))
            .map(|s| s.version)
            .unwrap_or_default();
        if !crate::pair::version_at_least(&version, VXLAN_SINCE) {
            return Err(format!("old_agent_vxlan:{server}:{version}"));
        }
    }
    let status: NetStatus = hub
        .ask_as(server, &Request::NetStatus)
        .await
        .map_err(|_| format!("offline:{server}"))?;
    match kind {
        LinkKind::Gre if !status.gre => Err(format!("no_gre:{server}")),
        LinkKind::Vxlan if !status.vxlan => Err(format!("no_vxlan:{server}")),
        _ => Ok(()),
    }
}

/// Checks that each server is connected, knows its public address, and can make links of
/// `kind`.
async fn check_servers(hub: &Hub, servers: &[&str], kind: LinkKind) -> Result<(), String> {
    for server in servers {
        if hub.addr_of(server).is_none() {
            return Err(format!("no_address:{server}"));
        }
        check_kind(hub, server, kind).await?;
    }
    Ok(())
}

/// Makes the link between `x` and `y` in `network`, or finds the one that is already
/// there. Returns it, and whether it was made now.
pub async fn ensure_link(
    hub: &Hub,
    op: &str,
    network: &str,
    x: &str,
    y: &str,
) -> Result<(Link, bool), String> {
    let (a, b) = if x < y { (x, y) } else { (y, x) };
    if let Ok(existing) = hub.networks.links(Some(network)) {
        if let Some(link) = existing.into_iter().find(|l| l.a == a && l.b == b) {
            return Ok((link, false));
        }
    }
    let ops = &hub.ops;
    let step_fail = |e: String| {
        ops.end(op, false, Some(e.clone()));
        e
    };
    let kind = hub
        .networks
        .networks()
        .ok()
        .and_then(|all| all.into_iter().find(|n| n.id == network))
        .map(|n| n.kind)
        .unwrap_or_default();
    ops.run(op, "net_check");
    if let Err(e) = check_servers(hub, &[a, b], kind).await {
        return Err(step_fail(e));
    }
    ops.end(op, true, None);

    ops.run(op, "net_alloc");
    let link =
        match hub
            .networks
            .create_links(network, &[(a.to_owned(), b.to_owned())], &hub.routes())
        {
            Ok(mut made) => made.remove(0),
            Err(e) => return Err(step_fail(format!("{e:#}"))),
        };
    ops.end(op, true, None);

    ops.run(op, "net_up");
    if let Err(e) = bring_up(hub, &link).await {
        let _ = tear_down(hub, &link).await;
        return Err(step_fail(e));
    }
    ops.end(op, true, None);
    Ok((link, true))
}

/// Starts making the links of a network for `servers`: a full mesh, or one link from
/// `hub_server` to each of the others.
pub fn create_links(
    hub: &Arc<Hub>,
    network: &str,
    servers: Vec<String>,
    hub_server: Option<String>,
) -> Result<String> {
    if servers.len() < 2 || hub_server.as_ref().is_some_and(|h| !servers.contains(h)) {
        bail!("bad_input");
    }
    let id = hub.ops.begin("network", network)?;
    let hub = hub.clone();
    let (op, network) = (id.clone(), network.to_owned());
    tokio::spawn(async move {
        let mut error = None;
        for (a, b) in mesh(&servers, hub_server.as_deref()) {
            if let Err(e) = ensure_link(&hub, &op, &network, &a, &b).await {
                error = Some(e);
                break;
            }
        }
        hub.ops.finish(&op, error, None);
    });
    Ok(id)
}

/// Whether `link` may go. A tunnel that listens on or dials one of its addresses keeps it in
/// use (`in_use:TUNNEL`). So does a server whose agent reaches the panel through it, or that
/// the panel dials across it (`agent_uses:SERVER`): the panel would lose the only way it has
/// to ask that server anything. `force` lets the link go anyway when that server is offline
/// (it may be gone for good, and the admin must never be left unable to delete).
fn may_go(hub: &Hub, link: &Link, views: &[crate::hub::ServerView], force: bool) -> Result<()> {
    for server in views {
        for t in &server.tunnels {
            let uses = |addr: &Option<String>| {
                addr.as_deref().is_some_and(|a| {
                    a.rsplit_once(':')
                        .is_some_and(|(h, _)| h == link.addr_a || h == link.addr_b)
                })
            };
            if uses(&t.info.listen) || uses(&t.info.remote) {
                bail!("in_use:{}", t.info.name);
            }
        }
        let reached = server.gre.as_ref().is_some_and(|g| g.link == link.id)
            || server
                .reverse
                .as_ref()
                .is_some_and(|r| r.host == link.addr_a || r.host == link.addr_b);
        if reached && !(force && !hub.is_online(&server.id)) {
            bail!("agent_uses:{}", server.id);
        }
    }
    Ok(())
}

/// Removes a link (see [`may_go`] for what keeps one). Returns the servers that remove its
/// interface later ([`tear_down`]).
pub async fn delete_link(hub: &Hub, id: &str, force: bool) -> Result<Vec<String>> {
    let Some(link) = hub.networks.links(None)?.into_iter().find(|l| l.id == id) else {
        bail!("no_such_link");
    };
    may_go(hub, &link, &hub.snapshot()?, force)?;
    tear_down(hub, &link).await
}

/// Removes a network and every link in it, all or nothing: if one link may not go, none
/// does. The interfaces are removed from every server at the same time; the servers that
/// remove them later come back, once each.
pub async fn delete_network(hub: &Arc<Hub>, id: &str, force: bool) -> Result<Vec<String>> {
    if !hub.networks.networks()?.iter().any(|n| n.id == id) {
        bail!("no_such_network");
    }
    let links = hub.networks.links(Some(id))?;
    let views = hub.snapshot()?;
    for link in &links {
        may_go(hub, link, &views, force)?;
    }
    let mut servers = Vec::new();
    for link in &links {
        hub.networks.delete_link(&link.id)?;
        servers.extend([link.a.clone(), link.b.clone()]);
    }
    hub.networks.delete_network(id)?;
    servers.sort();
    servers.dedup();
    let mut set = tokio::task::JoinSet::new();
    for server in servers {
        let hub = hub.clone();
        set.spawn(async move {
            let done = hub.net_flush(&server).await;
            (server, done)
        });
    }
    let mut left = Vec::new();
    while let Some(Ok((server, done))) = set.join_next().await {
        if let Err(e) = done {
            tracing::info!(server = %server, error = %format!("{e:#}"), "a deleted network's interfaces are removed later");
            left.push(server);
        }
    }
    left.sort();
    Ok(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link {
            id: "x".into(),
            network: "n".into(),
            a: "aaa".into(),
            b: "bbb".into(),
            subnet: "10.77.0.0/30".into(),
            addr_a: "10.77.0.1".into(),
            addr_b: "10.77.0.2".into(),
            gre_key: 1,
            ifname: "kz-12345".into(),
            created: 0,
            kind: LinkKind::Gre,
            port: 0,
        }
    }

    #[test]
    fn a_tunnel_over_a_link_uses_the_exits_end_and_the_wizards_port() {
        assert_eq!(
            tunnel_endpoint(&link(), "bbb", "0.0.0.0:3080"),
            "10.77.0.2:3080"
        );
        assert_eq!(tunnel_endpoint(&link(), "aaa", "[::]:443"), "10.77.0.1:443");
    }

    #[test]
    fn the_accepting_server_is_the_exit_when_direct_and_the_entry_when_reverse() {
        let req = |mode: &str| crate::pair::PairRequest {
            name: "t".into(),
            entry: "aaa".into(),
            exit: "bbb".into(),
            mode: mode.into(),
            transport: "tcpmux".into(),
            profile: None,
            listen: "0.0.0.0:3080".into(),
            dial: String::new(),
            pool: None,
            ws_path: None,
            ws_host: None,
            tls_sni: None,
            mux: None,
            encryption: None,
            quic_obfs: false,
            max_conns_per_ip: None,
            udp_max_flows: None,
            proxy_protocol: None,
            tls_cert: None,
            tls_key: None,
            tls_host: None,
            forwards: Vec::new(),
            rotate: false,
            network: Some("n".into()),
        };
        let at = |mode: &str| {
            let r = req(mode);
            let [acceptor, _] = r.acceptor_first();
            tunnel_endpoint(&link(), acceptor, &r.listen)
        };
        assert_eq!(at("direct"), "10.77.0.2:3080", "the exit (bbb) listens");
        assert_eq!(at("reverse"), "10.77.0.1:3080", "the entry (aaa) listens");
    }

    #[test]
    fn each_end_of_a_link_gets_its_own_side_of_the_addresses() {
        let a = spec_for(&link(), "aaa", "192.0.2.1", "192.0.2.2");
        let b = spec_for(&link(), "bbb", "192.0.2.2", "192.0.2.1");
        assert_eq!(
            (a.address.as_str(), a.peer.as_str()),
            ("10.77.0.1", "10.77.0.2")
        );
        assert_eq!(
            (b.address.as_str(), b.peer.as_str()),
            ("10.77.0.2", "10.77.0.1")
        );
        assert_eq!((a.name.as_str(), a.key, a.prefix), ("kz-12345", 1, 30));
        assert!(crate::net::validate(&a).is_ok() && crate::net::validate(&b).is_ok());
        assert_eq!((a.kind, a.port, a.mtu), (LinkKind::Gre, 0, 1476));
    }

    #[test]
    fn a_vxlan_links_spec_carries_its_vni_port_and_smaller_mtu() {
        let l = Link {
            kind: LinkKind::Vxlan,
            port: 8472,
            gre_key: 9,
            ..link()
        };
        let a = spec_for(&l, "aaa", "192.0.2.1", "192.0.2.2");
        assert_eq!(
            (a.kind, a.port, a.key, a.mtu),
            (LinkKind::Vxlan, 8472, 9, 1450)
        );
        assert!(crate::net::validate(&a).is_ok());
    }

    #[test]
    fn only_unicast_ipv4_addresses_are_accepted_for_a_server() {
        for ok in ["192.0.2.1", "203.0.113.5", "10.0.0.7"] {
            assert!(valid_addr(ok), "{ok}");
        }
        for bad in [
            "",
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "host.example",
            "1.2.3",
        ] {
            assert!(!valid_addr(bad), "{bad}");
        }
    }
}
