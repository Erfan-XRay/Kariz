//! Private networks as operations: making the links of a network (with their interfaces on
//! both servers and a path test), removing them, and keeping each server's list of links
//! in step with the panel's.
//!
//! Like tunnels (`pair.rs`) these run in the background as steps the browser follows, and
//! a step that fails undoes what the earlier ones did.

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{bail, Result};

use crate::hub::Hub;
use crate::networks::{mesh, Link};
use crate::wire::{Ack, NetSpec, NetStatus, PingReply, Request};

/// The MTU of a GRE link on a 1500-byte path.
const MTU: u32 = crate::net::DEFAULT_MTU;

/// Whether `text` is an address a server can be reached at: IPv4, unicast.
pub fn valid_addr(text: &str) -> bool {
    text.parse::<Ipv4Addr>().is_ok_and(|a| {
        !(a.is_unspecified() || a.is_loopback() || a.is_multicast() || a.is_broadcast())
    })
}

/// One end of a link, as the server that owns it is told: `mine` is the address of this
/// end, `theirs` the other's.
fn spec_for(link: &Link, server: &str, local: &str, remote: &str) -> NetSpec {
    let first = link.a == server;
    NetSpec {
        name: link.ifname.clone(),
        local: local.to_owned(),
        remote: remote.to_owned(),
        key: link.gre_key,
        address: if first { &link.addr_a } else { &link.addr_b }.clone(),
        peer: if first { &link.addr_b } else { &link.addr_a }.clone(),
        prefix: 30,
        mtu: MTU,
    }
}

/// The address a tunnel over `link` listens on and dials: the exit's end of the link (in
/// direct mode the exit listens), with the port of the wizard's listen address.
pub fn tunnel_endpoint(link: &Link, exit: &str, listen: &str) -> String {
    let addr = if link.a == exit {
        &link.addr_a
    } else {
        &link.addr_b
    };
    format!("{addr}:{}", listen.rsplit(':').next().unwrap_or("0"))
}

impl Hub {
    /// The specs of every link of `server` whose two public addresses are known.
    pub fn net_specs(&self, server: &str) -> Result<Vec<NetSpec>> {
        let mut out = Vec::new();
        for link in self.networks.links(None)? {
            if link.a != server && link.b != server {
                continue;
            }
            let other = if link.a == server { &link.b } else { &link.a };
            let (Some(local), Some(remote)) = (self.addr_of(server), self.addr_of(other)) else {
                continue;
            };
            out.push(spec_for(&link, server, &local, &remote));
        }
        Ok(out)
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

/// Removes a link's interface from both ends (a server that is offline is put right when
/// it connects) and its row.
pub async fn tear_down(hub: &Hub, link: &Link) {
    hub.networks.delete_link(&link.id).ok();
    for server in [&link.a, &link.b] {
        let _ = hub.net_sync(server).await;
    }
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
        Err("gre_blocked".to_owned())
    }
}

/// Checks that each server is connected, knows its public address, and can make GRE.
async fn check_servers(hub: &Hub, servers: &[&str]) -> Result<(), String> {
    for server in servers {
        if hub.addr_of(server).is_none() {
            return Err(format!("no_address:{server}"));
        }
        let status: NetStatus = hub
            .ask_as(server, &Request::NetStatus)
            .await
            .map_err(|_| format!("offline:{server}"))?;
        if !status.gre {
            return Err(format!("no_gre:{server}"));
        }
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
    ops.run(op, "net_check");
    if let Err(e) = check_servers(hub, &[a, b]).await {
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
        tear_down(hub, &link).await;
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

/// Removes a link. A tunnel that listens on or dials one of its addresses keeps it in use.
pub async fn delete_link(hub: &Hub, id: &str) -> Result<()> {
    let Some(link) = hub.networks.links(None)?.into_iter().find(|l| l.id == id) else {
        bail!("no_such_link");
    };
    for server in hub.snapshot()? {
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
    }
    tear_down(hub, &link).await;
    Ok(())
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
