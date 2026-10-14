//! Private networks between the servers: a network is a pool
//! of private IPv4 addresses the panel owns, and every link between two servers takes its
//! own /30 from it, so that **no address is ever given twice**.
//!
//! The rules live here and in the schema together: the panel checks them (and says which
//! one a request broke), and SQLite's `UNIQUE` on every subnet and address makes a duplicate
//! impossible even if two requests were to arrive at the same moment.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

use anyhow::{anyhow, bail, Result};
use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

use crate::auth::now;
use crate::config::random_hex;
use crate::db::Db;
use crate::net::{VNI_MAX, VXLAN_PORT};
use crate::wire::LinkKind;

/// An IPv4 network: the address of its first host bit pattern, and how many bits are fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    pub net: u32,
    pub prefix: u8,
}

impl Cidr {
    /// `10.77.0.0/16`. The address must be the network's own (no host bits set).
    pub fn parse(text: &str) -> Result<Self> {
        let (addr, prefix) = text
            .trim()
            .split_once('/')
            .ok_or_else(|| anyhow!("bad_cidr"))?;
        let addr: Ipv4Addr = addr.parse().map_err(|_| anyhow!("bad_cidr"))?;
        let prefix: u8 = prefix.parse().map_err(|_| anyhow!("bad_cidr"))?;
        if prefix > 32 {
            bail!("bad_cidr");
        }
        let net = u32::from(addr);
        let cidr = Self { net, prefix };
        if net & !cidr.mask() != 0 {
            bail!("bad_cidr");
        }
        Ok(cidr)
    }

    fn mask(self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        }
    }

    pub fn first(self) -> u32 {
        self.net
    }

    pub fn last(self) -> u32 {
        self.net | !self.mask()
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.first() <= other.last() && other.first() <= self.last()
    }

    pub fn contains(self, addr: u32) -> bool {
        self.first() <= addr && addr <= self.last()
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", Ipv4Addr::from(self.net), self.prefix)
    }
}

/// The private ranges a pool may live in.
const PRIVATE: [(&str, u8); 4] = [
    ("10.0.0.0", 8),
    ("172.16.0.0", 12),
    ("192.168.0.0", 16),
    ("100.64.0.0", 10),
];

/// A pool must be inside a private range and no smaller than a /24.
pub fn check_pool(pool: Cidr) -> Result<()> {
    let inside = PRIVATE.iter().any(|(net, prefix)| {
        let range = Cidr {
            net: u32::from(net.parse::<Ipv4Addr>().unwrap_or(Ipv4Addr::UNSPECIFIED)),
            prefix: *prefix,
        };
        range.contains(pool.first()) && range.contains(pool.last())
    });
    if !inside {
        bail!("not_private");
    }
    if pool.prefix > 24 {
        bail!("too_small");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub cidr: String,
    /// How its links are made, and for VXLAN the UDP port (0 for GRE).
    pub kind: LinkKind,
    pub port: u16,
    pub created: i64,
    /// Links that take a /30 of it now, and how many it can hold at most.
    pub links: u32,
    pub capacity: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub network: String,
    /// The two servers (ids), in a fixed order: `a` sorts before `b`.
    pub a: String,
    pub b: String,
    pub subnet: String,
    pub addr_a: String,
    pub addr_b: String,
    /// Tells two GRE tunnels between the same two public addresses apart; for VXLAN, the
    /// VNI (no two VXLAN links of a server share one).
    pub gre_key: u32,
    /// The interface name on both servers (`kz-` and five hex digits).
    pub ifname: String,
    pub created: i64,
    /// Its network's kind and port.
    pub kind: LinkKind,
    pub port: u16,
}

/// Which servers a link joins, and how a set of servers is joined.
pub fn mesh(servers: &[String], hub: Option<&str>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match hub {
        Some(h) => {
            for s in servers.iter().filter(|s| s.as_str() != h) {
                out.push((h.to_owned(), s.clone()));
            }
        }
        None => {
            for (i, a) in servers.iter().enumerate() {
                for b in &servers[i + 1..] {
                    out.push((a.clone(), b.clone()));
                }
            }
        }
    }
    out
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The routes each server reports, as parsed networks, keyed by server id.
pub type Routes = HashMap<String, Vec<String>>;

fn parsed(list: &[String]) -> Vec<Cidr> {
    list.iter().filter_map(|r| Cidr::parse(r).ok()).collect()
}

pub struct Networks {
    db: Db,
}

impl Networks {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Makes a GRE network. `routes` are what the connected servers already route
    /// (`(server name, its networks)`), so the pool never overlaps one of them.
    pub fn create_network(
        &self,
        name: &str,
        cidr: &str,
        routes: &[(String, Vec<String>)],
    ) -> Result<Network> {
        self.create_network_of(name, cidr, LinkKind::Gre, 0, routes)
    }

    /// Makes a network whose links are `kind`; a VXLAN one runs over UDP `port` (0: the
    /// usual 4789). Errors as [`Self::create_network`], and `bad_port`.
    pub fn create_network_of(
        &self,
        name: &str,
        cidr: &str,
        kind: LinkKind,
        port: u16,
        routes: &[(String, Vec<String>)],
    ) -> Result<Network> {
        if !valid_name(name) {
            bail!("bad_name");
        }
        let port = match kind {
            LinkKind::Gre if port != 0 => bail!("bad_port"),
            LinkKind::Gre => 0,
            LinkKind::Vxlan if port == 0 => VXLAN_PORT,
            LinkKind::Vxlan => port,
        };
        let pool = Cidr::parse(cidr)?;
        check_pool(pool)?;
        for (server, list) in routes {
            if let Some(r) = parsed(list).into_iter().find(|r| r.overlaps(pool)) {
                bail!("overlaps_route:{server}:{r}");
            }
        }
        let mut conn = self.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare("SELECT name, cidr FROM networks")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (other, other_cidr) = row?;
                if other == name {
                    bail!("name_taken");
                }
                if Cidr::parse(&other_cidr).is_ok_and(|o| o.overlaps(pool)) {
                    bail!("overlaps_network:{other}");
                }
            }
        }
        let id = random_hex(6)?;
        tx.execute(
            "INSERT INTO networks (id, name, cidr, created, kind, port) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, name, pool.to_string(), now(), kind.as_str(), port],
        )?;
        tx.commit()?;
        drop(conn);
        self.network(&id)
    }

    fn capacity(pool: Cidr) -> u32 {
        1u32 << (30 - pool.prefix.min(30))
    }

    fn network(&self, id: &str) -> Result<Network> {
        self.networks()?
            .into_iter()
            .find(|n| n.id == id)
            .ok_or_else(|| anyhow!("no_such_network"))
    }

    pub fn networks(&self) -> Result<Vec<Network>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.cidr, n.created, (SELECT COUNT(*) FROM net_links l WHERE l.network = n.id),
                    n.kind, n.port
             FROM networks n ORDER BY n.created, n.id",
        )?;
        let rows = stmt.query_map([], |r| {
            let cidr: String = r.get(2)?;
            Ok(Network {
                id: r.get(0)?,
                name: r.get(1)?,
                capacity: Cidr::parse(&cidr).map_or(0, Self::capacity),
                cidr,
                created: r.get(3)?,
                links: r.get(4)?,
                kind: kind_of(&r.get::<_, String>(5)?),
                port: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn links(&self, network: Option<&str>) -> Result<Vec<Link>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "SELECT l.id, l.network, l.a, l.b, l.subnet, l.addr_a, l.addr_b, l.gre_key, l.ifname,
                    l.created, n.kind, n.port
             FROM net_links l JOIN networks n ON n.id = l.network
             WHERE (?1 IS NULL OR l.network = ?1) ORDER BY l.created, l.id",
        )?;
        let rows = stmt.query_map([network], |r| {
            Ok(Link {
                id: r.get(0)?,
                network: r.get(1)?,
                a: r.get(2)?,
                b: r.get(3)?,
                subnet: r.get(4)?,
                addr_a: r.get(5)?,
                addr_b: r.get(6)?,
                gre_key: r.get(7)?,
                ifname: r.get(8)?,
                created: r.get(9)?,
                kind: kind_of(&r.get::<_, String>(10)?),
                port: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Gives every pair its own /30 of the network, all or nothing. A subnet that overlaps
    /// a route either of its two servers reports is skipped, so the pair never gets an
    /// address the server already uses for something else. Errors: `no_such_network`,
    /// `same_server`, `pool_full`.
    pub fn create_links(
        &self,
        network: &str,
        pairs: &[(String, String)],
        routes: &Routes,
    ) -> Result<Vec<Link>> {
        let mut conn = self.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (cidr, kind, port): (String, String, u16) = tx
            .query_row(
                "SELECT cidr, kind, port FROM networks WHERE id = ?1",
                [network],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| anyhow!("no_such_network"))?;
        let kind = kind_of(&kind);
        let pool = Cidr::parse(&cidr)?;
        let mut used: HashSet<u32> = {
            let mut stmt = tx.prepare("SELECT subnet FROM net_links WHERE network = ?1")?;
            let rows = stmt.query_map([network], |r| r.get::<_, String>(0))?;
            rows.filter_map(|s| s.ok().and_then(|s| Cidr::parse(&s).ok()))
                .map(|c| c.net)
                .collect()
        };
        let mut made = Vec::new();
        for (x, y) in pairs {
            if x == y {
                bail!("same_server");
            }
            let (a, b) = if x < y { (x, y) } else { (y, x) };
            let avoid: Vec<Cidr> = [a, b]
                .iter()
                .flat_map(|s| parsed(routes.get(*s).map_or(&[][..], Vec::as_slice)))
                .collect();
            let subnet = (0..Self::capacity(pool))
                .map(|i| pool.net + i * 4)
                .find(|base| {
                    let block = Cidr {
                        net: *base,
                        prefix: 30,
                    };
                    !used.contains(base) && !avoid.iter().any(|r| r.overlaps(block))
                })
                .ok_or_else(|| anyhow!("pool_full"))?;
            used.insert(subnet);
            // GRE: the smallest key this pair has not used yet. VXLAN: the smallest VNI that
            // no link of either server has (the kernel takes a VNI once per UDP port on a
            // server, whoever the peer is); it is also new for the pair, as the schema wants.
            let taken: HashSet<u32> = {
                let sql = match kind {
                    LinkKind::Gre => "SELECT gre_key FROM net_links WHERE a = ?1 AND b = ?2",
                    LinkKind::Vxlan => {
                        "SELECT gre_key FROM net_links WHERE a IN (?1, ?2) OR b IN (?1, ?2)"
                    }
                };
                let mut stmt = tx.prepare(sql)?;
                let rows = stmt.query_map(params![a, b], |r| r.get::<_, u32>(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let gre_key = (1u32..=VNI_MAX)
                .find(|k| !taken.contains(k))
                .ok_or_else(|| anyhow!("pool_full"))?;
            let id = random_hex(6)?;
            let link = Link {
                ifname: format!("kz-{}", &id[..5]),
                id,
                network: network.to_owned(),
                a: a.clone(),
                b: b.clone(),
                subnet: Cidr {
                    net: subnet,
                    prefix: 30,
                }
                .to_string(),
                addr_a: Ipv4Addr::from(subnet + 1).to_string(),
                addr_b: Ipv4Addr::from(subnet + 2).to_string(),
                gre_key,
                created: now(),
                kind,
                port,
            };
            tx.execute(
                "INSERT INTO net_links (id, network, a, b, subnet, addr_a, addr_b, gre_key, ifname, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    link.id, link.network, link.a, link.b, link.subnet, link.addr_a, link.addr_b,
                    link.gre_key, link.ifname, link.created
                ],
            )?;
            made.push(link);
        }
        tx.commit()?;
        Ok(made)
    }

    pub fn delete_link(&self, id: &str) -> Result<bool> {
        Ok(self
            .db
            .conn()
            .execute("DELETE FROM net_links WHERE id = ?1", [id])?
            > 0)
    }

    /// A network with links in it cannot be deleted: its addresses are in use.
    pub fn delete_network(&self, id: &str) -> Result<bool> {
        let conn = self.db.conn();
        let used: i64 = conn.query_row(
            "SELECT COUNT(*) FROM net_links WHERE network = ?1",
            [id],
            |r| r.get(0),
        )?;
        if used > 0 {
            bail!("in_use");
        }
        Ok(conn.execute("DELETE FROM networks WHERE id = ?1", [id])? > 0)
    }
}

/// A kind as the database keeps it; anything else is the GRE it was before there was a kind.
fn kind_of(text: &str) -> LinkKind {
    LinkKind::parse(text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets() -> Networks {
        Networks::new(Db::in_memory().unwrap())
    }

    fn why<T>(r: Result<T>) -> String {
        format!("{:#}", r.err().expect("an error"))
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn cidr_parsing_and_overlap() {
        let a = Cidr::parse("10.77.0.0/16").unwrap();
        assert_eq!(a.to_string(), "10.77.0.0/16");
        assert!(a.overlaps(Cidr::parse("10.77.5.0/24").unwrap()));
        assert!(a.overlaps(Cidr::parse("10.0.0.0/8").unwrap()));
        assert!(!a.overlaps(Cidr::parse("10.78.0.0/16").unwrap()));
        for bad in [
            "10.77.0.1/16",
            "10.77.0.0",
            "10.77.0.0/33",
            "x/16",
            "10.77.0.0/-1",
            "",
        ] {
            assert!(Cidr::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_pool_is_private_and_not_smaller_than_a_slash_24() {
        let n = nets();
        for ok in [
            "10.77.0.0/16",
            "172.20.0.0/16",
            "192.168.5.0/24",
            "100.64.0.0/10",
            "10.0.0.0/8",
        ] {
            assert!(
                n.create_network(&format!("n{}", ok.replace(['.', '/'], "")), ok, &[])
                    .is_ok(),
                "{ok}"
            );
            n.delete_network(&n.networks().unwrap().pop().unwrap().id)
                .unwrap();
        }
        assert_eq!(why(n.create_network("a", "8.8.8.0/24", &[])), "not_private");
        assert_eq!(
            why(n.create_network("a", "172.32.0.0/16", &[])),
            "not_private"
        );
        assert_eq!(why(n.create_network("a", "11.0.0.0/8", &[])), "not_private");
        assert_eq!(why(n.create_network("a", "10.0.0.0/7", &[])), "not_private");
        assert_eq!(why(n.create_network("a", "10.77.0.0/25", &[])), "too_small");
        assert_eq!(
            why(n.create_network("a b", "10.77.0.0/16", &[])),
            "bad_name"
        );
        assert_eq!(why(n.create_network("a", "10.77.0.1/16", &[])), "bad_cidr");
    }

    #[test]
    fn two_networks_may_not_overlap_or_share_a_name() {
        let n = nets();
        n.create_network("main", "10.77.0.0/16", &[]).unwrap();
        assert_eq!(
            why(n.create_network("main", "10.78.0.0/16", &[])),
            "name_taken"
        );
        assert_eq!(
            why(n.create_network("b", "10.77.4.0/24", &[])),
            "overlaps_network:main"
        );
        assert_eq!(
            why(n.create_network("c", "10.0.0.0/8", &[])),
            "overlaps_network:main"
        );
        n.create_network("d", "10.78.0.0/16", &[]).unwrap();
    }

    #[test]
    fn a_pool_never_overlaps_what_a_server_already_routes() {
        let n = nets();
        let routes = vec![
            ("tehran-1".to_string(), vec!["172.17.0.0/16".to_string()]),
            ("frankfurt".to_string(), vec!["10.77.3.0/24".to_string()]),
        ];
        assert_eq!(
            why(n.create_network("main", "10.77.0.0/16", &routes)),
            "overlaps_route:frankfurt:10.77.3.0/24"
        );
        n.create_network("main", "10.88.0.0/16", &routes).unwrap();
    }

    #[test]
    fn every_link_gets_its_own_subnet_and_no_address_repeats() {
        let n = nets();
        let net = n.create_network("main", "10.77.0.0/24", &[]).unwrap();
        assert_eq!(net.capacity, 64);
        let servers = ids(&["a", "b", "c", "d"]);
        let links = n
            .create_links(&net.id, &mesh(&servers, None), &Routes::new())
            .unwrap();
        assert_eq!(links.len(), 6, "a full mesh of four is six links");
        let mut addresses: Vec<&str> = links
            .iter()
            .flat_map(|l| [l.addr_a.as_str(), l.addr_b.as_str()])
            .collect();
        let count = addresses.len();
        addresses.sort_unstable();
        addresses.dedup();
        assert_eq!(addresses.len(), count, "twelve different addresses");
        let mut subnets: Vec<&str> = links.iter().map(|l| l.subnet.as_str()).collect();
        subnets.sort_unstable();
        subnets.dedup();
        assert_eq!(subnets.len(), 6);
        assert_eq!(links[0].subnet, "10.77.0.0/30");
        assert_eq!(
            (links[0].addr_a.as_str(), links[0].addr_b.as_str()),
            ("10.77.0.1", "10.77.0.2")
        );
        assert_eq!(links[1].subnet, "10.77.0.4/30");
        assert!(links
            .iter()
            .all(|l| l.a < l.b && l.ifname.starts_with("kz-") && l.ifname.len() == 8));
        assert_eq!(n.networks().unwrap()[0].links, 6);
    }

    #[test]
    fn hub_and_spoke_has_one_link_per_spoke() {
        let servers = ids(&["a", "b", "c", "d"]);
        assert_eq!(mesh(&servers, Some("b")).len(), 3);
        assert!(mesh(&servers, Some("b")).iter().all(|(h, _)| h == "b"));
        assert_eq!(mesh(&servers, None).len(), 6);
        assert!(mesh(&ids(&["a"]), None).is_empty());
    }

    #[test]
    fn a_second_link_between_the_same_pair_gets_another_key_and_subnet() {
        let n = nets();
        let net = n.create_network("main", "10.77.0.0/16", &[]).unwrap();
        let pair = vec![("a".to_string(), "b".to_string())];
        let first = n.create_links(&net.id, &pair, &Routes::new()).unwrap();
        let second = n.create_links(&net.id, &pair, &Routes::new()).unwrap();
        let third = n
            .create_links(
                &net.id,
                &[("b".to_string(), "a".to_string())],
                &Routes::new(),
            )
            .unwrap();
        assert_eq!(
            (first[0].gre_key, second[0].gre_key, third[0].gre_key),
            (1, 2, 3)
        );
        assert_ne!(first[0].subnet, second[0].subnet);
        // freeing the first key lets the next link take it again
        n.delete_link(&first[0].id).unwrap();
        let again = n.create_links(&net.id, &pair, &Routes::new()).unwrap();
        assert_eq!(again[0].gre_key, 1);
        assert_eq!(
            again[0].subnet, first[0].subnet,
            "a freed subnet is used again"
        );
    }

    #[test]
    fn no_two_vxlan_links_of_a_server_share_a_vni() {
        let n = nets();
        let vx = n
            .create_network_of("vx", "10.77.0.0/16", LinkKind::Vxlan, 0, &[])
            .unwrap();
        assert_eq!((vx.kind, vx.port), (LinkKind::Vxlan, 4789));
        let link = |x: &str, y: &str| {
            n.create_links(&vx.id, &[(x.into(), y.into())], &Routes::new())
                .unwrap()
                .remove(0)
        };
        // The kernel takes a VNI once per port on a server, whoever the peer is: a-b and
        // a-c may not share one, while c-d (no server in common with a-b) may.
        let ab = link("a", "b");
        let ac = link("a", "c");
        let cd = link("c", "d");
        let bd = link("b", "d");
        assert_eq!((ab.gre_key, ac.gre_key), (1, 2));
        assert_eq!(cd.gre_key, 1, "c-d shares no server with a-b");
        assert_eq!(bd.gre_key, 2, "b has 1, d has 1: the next free one");
        assert!([&ab, &ac, &cd, &bd]
            .iter()
            .all(|l| l.kind == LinkKind::Vxlan && l.port == 4789));
        // GRE keys stay per pair, as before
        let gre = n.create_network("g", "10.78.0.0/16", &[]).unwrap();
        let g = n
            .create_links(&gre.id, &[("c".into(), "e".into())], &Routes::new())
            .unwrap();
        assert_eq!((g[0].kind, g[0].gre_key, g[0].port), (LinkKind::Gre, 1, 0));
        // and the list shows each network's kind (two made in the same second come in any order)
        let mut kinds: Vec<_> = n
            .networks()
            .unwrap()
            .into_iter()
            .map(|x| (x.name, x.kind))
            .collect();
        kinds.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(
            kinds,
            vec![("g".into(), LinkKind::Gre), ("vx".into(), LinkKind::Vxlan)]
        );
    }

    #[test]
    fn a_network_kind_takes_its_own_port_rules() {
        let n = nets();
        assert_eq!(
            why(n.create_network_of("g", "10.77.0.0/16", LinkKind::Gre, 4789, &[])),
            "bad_port"
        );
        let v = n
            .create_network_of("v", "10.77.0.0/16", LinkKind::Vxlan, 8472, &[])
            .unwrap();
        assert_eq!(v.port, 8472);
        let g = n.create_network("g", "10.78.0.0/16", &[]).unwrap();
        assert_eq!((g.kind, g.port), (LinkKind::Gre, 0));
    }

    #[test]
    fn a_subnet_that_a_server_already_routes_is_skipped() {
        let n = nets();
        let net = n.create_network("main", "10.77.0.0/24", &[]).unwrap();
        let mut routes = Routes::new();
        // b routes 10.77.0.0-10.77.0.15, which covers the first four /30s
        routes.insert("b".into(), vec!["10.77.0.0/28".into()]);
        let l = n
            .create_links(&net.id, &[("a".to_string(), "b".to_string())], &routes)
            .unwrap();
        assert_eq!(l[0].subnet, "10.77.0.16/30");
        // a pair that does not include b is not held back by it
        let l = n
            .create_links(&net.id, &[("a".to_string(), "c".to_string())], &routes)
            .unwrap();
        assert_eq!(l[0].subnet, "10.77.0.0/30");
    }

    #[test]
    fn a_full_pool_and_a_failed_batch_change_nothing() {
        let n = nets();
        let net = n.create_network("main", "192.168.7.0/24", &[]).unwrap();
        // 64 /30s: fill it, then one more
        let servers: Vec<String> = (0..12).map(|i| format!("s{i:02}")).collect();
        let pairs = mesh(&servers, None); // 66 links, two too many
        assert_eq!(
            why(n.create_links(&net.id, &pairs, &Routes::new())),
            "pool_full"
        );
        assert!(
            n.links(None).unwrap().is_empty(),
            "the batch is all or nothing"
        );
        assert_eq!(
            n.create_links(&net.id, &pairs[..64], &Routes::new())
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            why(n.create_links(&net.id, &pairs[..1], &Routes::new())),
            "pool_full"
        );
        assert_eq!(
            why(n.create_links(&net.id, &[("a".into(), "a".into())], &Routes::new())),
            "same_server"
        );
        assert_eq!(
            why(n.create_links("nope", &pairs[..1], &Routes::new())),
            "no_such_network"
        );
    }

    #[test]
    fn requests_at_the_same_moment_never_share_an_address() {
        let db = Db::in_memory().unwrap();
        let n = std::sync::Arc::new(Networks::new(db));
        let net = n.create_network("main", "10.77.0.0/16", &[]).unwrap();
        let mut threads = Vec::new();
        for t in 0..8 {
            let (n, id) = (n.clone(), net.id.clone());
            threads.push(std::thread::spawn(move || {
                let mut got = Vec::new();
                for i in 0..10 {
                    let pair = (format!("s{t}"), format!("x{i}"));
                    got.extend(n.create_links(&id, &[pair], &Routes::new()).unwrap());
                }
                got
            }));
        }
        let all: Vec<Link> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(all.len(), 80);
        let set: HashSet<&str> = all
            .iter()
            .flat_map(|l| [l.addr_a.as_str(), l.addr_b.as_str()])
            .collect();
        assert_eq!(set.len(), 160, "160 different addresses");
        let names: HashSet<&str> = all.iter().map(|l| l.ifname.as_str()).collect();
        assert_eq!(names.len(), 80);
    }

    #[test]
    fn the_schema_itself_refuses_a_repeated_address() {
        let db = Db::in_memory().unwrap();
        let n = Networks::new(db.clone());
        let net = n.create_network("main", "10.77.0.0/16", &[]).unwrap();
        n.create_links(&net.id, &[("a".into(), "b".into())], &Routes::new())
            .unwrap();
        // even a write that skips every check cannot repeat a subnet or an address
        let dup = db.conn().execute(
            "INSERT INTO net_links (id, network, a, b, subnet, addr_a, addr_b, gre_key, ifname, created)
             VALUES ('x', ?1, 'c', 'd', '10.77.9.0/30', '10.77.0.1', '10.77.9.2', 1, 'kz-zzzzz', 0)",
            [&net.id],
        );
        assert!(dup.is_err(), "the address 10.77.0.1 is taken");
        let dup = db.conn().execute(
            "INSERT INTO net_links (id, network, a, b, subnet, addr_a, addr_b, gre_key, ifname, created)
             VALUES ('y', ?1, 'c', 'd', '10.77.0.0/30', '10.77.9.1', '10.77.9.2', 1, 'kz-yyyyy', 0)",
            [&net.id],
        );
        assert!(dup.is_err(), "the subnet is taken");
    }

    #[test]
    fn a_network_with_links_cannot_be_deleted() {
        let n = nets();
        let net = n.create_network("main", "10.77.0.0/16", &[]).unwrap();
        let l = n
            .create_links(&net.id, &[("a".into(), "b".into())], &Routes::new())
            .unwrap();
        assert_eq!(why(n.delete_network(&net.id)), "in_use");
        assert!(n.delete_link(&l[0].id).unwrap());
        assert!(!n.delete_link(&l[0].id).unwrap());
        assert!(n.delete_network(&net.id).unwrap());
    }
}
