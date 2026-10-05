//! Private networks through the hub and a real agent, with the agents' `ip` and `ping`
//! recorded instead of run (the real thing is `tests/gre_ns.sh`): a link is made on both
//! servers with the right addresses, key and public ends, reused for a second request,
//! undone when the path test fails, and refused while a tunnel uses it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kariz_panel::agent::{self, Agent};
use kariz_panel::db::Db;
use kariz_panel::hub::{Hub, LOCAL};
use kariz_panel::manage::{Fut, Systemd};
use kariz_panel::net::Exec;
use kariz_panel::netops;
use kariz_panel::pair::{self, PairRequest};

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    ping_fails: AtomicBool,
}

impl Fake {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Exec for Fake {
    fn run(&self, program: &str, args: Vec<String>) -> Fut<anyhow::Result<(bool, String)>> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.join(" ")));
        let ok = !(program == "ping" && self.ping_fails.load(Ordering::SeqCst));
        let out = if program == "ping" {
            "rtt min/avg/max/mdev = 1.0/1.5/2.0/0.2 ms\n".to_owned()
        } else {
            String::new()
        };
        Box::pin(async move { Ok((ok, out)) })
    }
}

async fn finished(hub: &Hub, id: &str) -> kariz_panel::pair::Op {
    for _ in 0..300 {
        let op = hub.ops.get(id).expect("the operation is known");
        if op.state != "running" {
            return op;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the operation did not finish");
}

#[tokio::test]
async fn links_are_made_on_both_servers_reused_undone_and_protected() {
    let root = tempfile::tempdir().unwrap();
    let (local_fake, agent_fake) = (Arc::new(Fake::default()), Arc::new(Fake::default()));
    let hub = Hub::with_options(
        Db::in_memory().unwrap(),
        root.path().join("kariz"),
        Arc::new(Systemd),
        Duration::from_secs(5),
        Some(root.path().join("panel")),
        Some(local_fake.clone()),
    );
    std::fs::create_dir_all(root.path().join("panel")).unwrap();
    let acceptor = kariz::link::Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
        .await
        .unwrap();
    let addr = acceptor.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(acceptor));

    let code = hub.create_join(Some("far-away"), &addr).unwrap();
    let agent_dir = root.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_file = agent_dir.join("agent.toml");
    let config = agent::enroll_from_code(&code, &agent_file).unwrap();
    config.save(&agent_file).unwrap();
    let agent = Agent::with_exec(&agent_file, config, Arc::new(Systemd), agent_fake.clone());
    tokio::spawn(agent.run());
    let mut remote = None;
    for _ in 0..100 {
        remote = hub
            .snapshot()
            .unwrap()
            .into_iter()
            .find(|s| !s.local && s.online)
            .map(|s| s.id);
        if remote.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let remote = remote.expect("the agent joined");
    let (a, b) = if LOCAL < remote.as_str() {
        (LOCAL.to_owned(), remote.clone())
    } else {
        (remote.clone(), LOCAL.to_owned())
    };

    // addresses: both ends need one, and it must be a unicast IPv4 address
    assert!(hub.set_addr(LOCAL, "127.0.0.1").is_err());
    assert!(hub.set_addr(LOCAL, "not an address").is_err());
    assert!(hub.set_addr("nobody", "192.0.2.9").is_err());
    let net = hub
        .networks
        .create_network("main", "10.77.0.0/24", &[])
        .unwrap();

    // without addresses the operation says which server lacks one, and makes nothing
    let both = vec![LOCAL.to_owned(), remote.clone()];
    let op = netops::create_links(&hub, &net.id, both.clone(), None).unwrap();
    let done = finished(&hub, &op).await;
    assert_eq!(done.state, "failed");
    assert!(done.error.unwrap().starts_with("no_address:"));
    assert!(hub.networks.links(None).unwrap().is_empty());

    hub.set_addr(LOCAL, "192.0.2.1").unwrap();
    hub.set_addr(&remote, "192.0.2.2").unwrap();
    assert_eq!(
        hub.snapshot()
            .unwrap()
            .iter()
            .find(|s| s.local)
            .unwrap()
            .addr
            .as_deref(),
        Some("192.0.2.1")
    );

    // ---- a link ----
    let op = netops::create_links(&hub, &net.id, both.clone(), None).unwrap();
    let done = finished(&hub, &op).await;
    assert_eq!(done.state, "done", "{done:?}");
    let links = hub.networks.links(None).unwrap();
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!((link.subnet.as_str(), link.gre_key), ("10.77.0.0/30", 1));
    assert_eq!((&link.a, &link.b), (&a, &b));
    let make = |fake: &Fake| {
        fake.calls()
            .into_iter()
            .find(|c| {
                c.starts_with("ip tunnel add kz-")
                    && !c.contains("kz-probe")
                    && c.contains("mode gre")
            })
            .expect("the interface was made")
    };
    // each side is the other's remote, and has its own private address
    let (local_line, remote_line) = (make(&local_fake), make(&agent_fake));
    assert!(
        local_line.contains("local 192.0.2.1 remote 192.0.2.2 key 1"),
        "{local_line}"
    );
    assert!(
        remote_line.contains("local 192.0.2.2 remote 192.0.2.1 key 1"),
        "{remote_line}"
    );
    let local_addr = if link.a == LOCAL {
        &link.addr_a
    } else {
        &link.addr_b
    };
    assert!(local_fake
        .calls()
        .iter()
        .any(|c| c == &format!("ip addr add {local_addr}/30 dev {}", link.ifname)));
    // the path was tested from the first server
    let pinged = |fake: &Fake| {
        fake.calls()
            .iter()
            .any(|c| c.starts_with("ping -c 3 -W 2 -I kz-"))
    };
    assert!(pinged(&local_fake) || pinged(&agent_fake));

    // ---- asking again reuses the link ----
    let op = netops::create_links(&hub, &net.id, both.clone(), None).unwrap();
    assert_eq!(finished(&hub, &op).await.state, "done");
    assert_eq!(hub.networks.links(None).unwrap().len(), 1);

    // ---- a tunnel over the link listens on the end of the server that accepts it ----
    let end = |server: &str| {
        if link.a == server {
            link.addr_a.clone()
        } else {
            link.addr_b.clone()
        }
    };
    let tunnel = |mode: &str, network: Option<String>| PairRequest {
        name: "over".into(),
        entry: LOCAL.into(),
        exit: remote.clone(),
        mode: mode.into(),
        transport: "tcpmux".into(),
        profile: None,
        listen: "0.0.0.0:3080".into(),
        dial: "203.0.113.5:3080".into(),
        pool: None,
        ws_path: None,
        ws_host: None,
        tls_sni: None,
        mux: None,
        encryption: None,
        quic_obfs: false,
        tls_cert: None,
        tls_key: None,
        tls_host: None,
        forwards: Vec::new(),
        rotate: false,
        network,
    };
    let mut made = None;
    let direct = pair::over_network(
        &hub,
        "x",
        &tunnel("direct", Some(net.id.clone())),
        &mut made,
    )
    .await
    .unwrap()
    .expect("a network was named");
    assert_eq!(
        direct.listen,
        format!("{}:3080", end(&remote)),
        "the exit listens"
    );
    assert_eq!(direct.dial, direct.listen);
    let reverse = pair::over_network(
        &hub,
        "x",
        &tunnel("reverse", Some(net.id.clone())),
        &mut made,
    )
    .await
    .unwrap()
    .expect("a network was named");
    assert_eq!(
        reverse.listen,
        format!("{}:3080", end(LOCAL)),
        "the entry listens"
    );
    assert_eq!(reverse.dial, reverse.listen);
    assert!(
        reverse.network.is_none() && made.is_none(),
        "the link was there already"
    );
    // No network: the request is used as it is (a tunnel taken off a network).
    assert!(
        pair::over_network(&hub, "x", &tunnel("reverse", None), &mut made)
            .await
            .unwrap()
            .is_none()
    );

    // ---- removing a link takes its interface down on both servers ----
    let (fake_a, fake_b) = (&local_fake, &agent_fake);
    let before = (fake_a.calls().len(), fake_b.calls().len());
    netops::delete_link(&hub, &link.id).await.unwrap();
    assert!(hub.networks.links(None).unwrap().is_empty());
    // both servers were told the new (empty) list and took the interface down
    for _ in 0..50 {
        if fake_a.calls().len() > before.0 && fake_b.calls().len() > before.1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(local_fake
        .calls()
        .iter()
        .any(|c| c == &format!("ip link del {}", link.ifname)));
    assert!(agent_fake
        .calls()
        .iter()
        .any(|c| c == &format!("ip link del {}", link.ifname)));

    // ---- a path that does not pass: undone on both servers, nothing kept ----
    local_fake.ping_fails.store(true, Ordering::SeqCst);
    agent_fake.ping_fails.store(true, Ordering::SeqCst);
    let op = netops::create_links(&hub, &net.id, both, None).unwrap();
    let done = finished(&hub, &op).await;
    assert_eq!(done.state, "failed");
    assert_eq!(done.error.as_deref(), Some("gre_blocked"));
    assert!(
        hub.networks.links(None).unwrap().is_empty(),
        "the link was removed"
    );
    assert_eq!(done.steps.last().unwrap().id, "net_up");

    // ---- removing a server takes its links with it ----
    local_fake.ping_fails.store(false, Ordering::SeqCst);
    agent_fake.ping_fails.store(false, Ordering::SeqCst);
    let op =
        netops::create_links(&hub, &net.id, vec![LOCAL.to_owned(), remote.clone()], None).unwrap();
    assert_eq!(finished(&hub, &op).await.state, "done");
    assert_eq!(hub.networks.links(None).unwrap().len(), 1);
    assert!(hub.remove(&remote).unwrap());
    assert!(hub.networks.links(None).unwrap().is_empty());
    assert_eq!(hub.addr_of(&remote), None);
}

/// *Add server* over a private network: the panel makes its own end of the GRE link at once
/// and lists the server as waiting; the code carries the other end, which the agent makes
/// before it dials the panel's private address. A reconnect code through that address
/// carries the link again.
#[tokio::test]
async fn a_server_added_over_gre_makes_its_end_from_the_code_and_comes_in() {
    let root = tempfile::tempdir().unwrap();
    let (local_fake, agent_fake) = (Arc::new(Fake::default()), Arc::new(Fake::default()));
    let hub = Hub::with_options(
        Db::in_memory().unwrap(),
        root.path().join("kariz"),
        Arc::new(Systemd),
        Duration::from_secs(5),
        Some(root.path().join("panel")),
        Some(local_fake.clone()),
    );
    std::fs::create_dir_all(root.path().join("panel")).unwrap();
    let acceptor = kariz::link::Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
        .await
        .unwrap();
    let addr = acceptor.local_addr().unwrap().to_string();
    let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
    tokio::spawn(hub.clone().serve_agents(acceptor));

    // The panel must know its own public address: GRE runs between the two.
    let why = |r: anyhow::Result<kariz_panel::hub::GreJoin>| format!("{:#}", r.unwrap_err());
    assert_eq!(hub.addr_of(LOCAL), None);
    assert_eq!(
        why(hub
            .create_gre_join(None, "192.0.2.2", None, None, port)
            .await),
        "no_address:local"
    );
    hub.set_addr(LOCAL, "192.0.2.1").unwrap();
    for (bad, code) in [
        ("not-an-ip", "bad_input"),
        ("127.0.0.1", "bad_input"),
        ("192.0.2.1", "same_address"),
    ] {
        assert_eq!(
            why(hub.create_gre_join(None, bad, None, None, port).await),
            code,
            "{bad}"
        );
    }
    assert!(hub.networks.networks().unwrap().is_empty());

    let made = hub
        .create_gre_join(Some("behind-gre"), "192.0.2.2", None, None, port)
        .await
        .unwrap();
    assert_eq!(
        (made.name.as_str(), made.network.as_str()),
        ("behind-gre", "kariz")
    );
    // A network was made for it, and the link took a /30 of it.
    let nets = hub.networks.networks().unwrap();
    assert_eq!((nets.len(), nets[0].cidr.as_str()), (1, "10.77.0.0/16"));
    let link = hub.networks.links(None).unwrap().remove(0);
    assert!(
        [&link.a, &link.b].contains(&&made.id) && [&link.a, &link.b].contains(&&LOCAL.to_owned())
    );
    // The panel's end is up already, from its public address to the new server's.
    assert!(
        local_fake.calls().iter().any(|c| c
            == &format!(
                "ip tunnel add {} mode gre local 192.0.2.1 remote 192.0.2.2 key 1 ttl 64",
                link.ifname
            )),
        "{:?}",
        local_fake.calls()
    );
    // Listed, waiting for its agent, as a server that comes in through the link.
    let listed = |hub: &Hub| {
        hub.snapshot()
            .unwrap()
            .into_iter()
            .find(|s| s.id == made.id)
            .unwrap()
    };
    let waiting = listed(&hub);
    assert!(!waiting.online && waiting.last_seen.is_none());
    assert_eq!(waiting.gre.map(|g| g.network).as_deref(), Some("kariz"));
    assert_eq!(waiting.addr.as_deref(), Some("192.0.2.2"));
    // The same address cannot be added twice: that server is reconnected instead.
    assert_eq!(
        why(hub
            .create_gre_join(None, "192.0.2.2", None, None, port)
            .await),
        format!("address_taken:{}", made.id)
    );

    // The code: the panel's private address, and the new server's end of the link.
    let code = kariz_panel::join::decode(&made.code).unwrap();
    assert_eq!(code.p, format!("{}:{port}", made.panel_addr));
    let theirs = code.g.clone().expect("the code carries the link");
    assert_eq!(
        (theirs.local.as_str(), theirs.remote.as_str(), theirs.key),
        ("192.0.2.2", "192.0.2.1", link.gre_key)
    );
    assert_eq!(
        (theirs.address.as_str(), theirs.peer.as_str()),
        (made.server_addr.as_str(), made.panel_addr.as_str())
    );
    assert_eq!(theirs.name, link.ifname);

    // ---- the agent joins with it ----
    let agent_dir = root.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_file = agent_dir.join("agent.toml");
    let mut config = agent::enroll_from_code(&made.code, &agent_file).unwrap();
    // Its end of the link is saved, to be made when the agent starts.
    let saved = std::fs::read_to_string(agent_dir.join("net.toml")).unwrap();
    assert!(
        saved.contains(&link.ifname) && saved.contains("192.0.2.1"),
        "{saved}"
    );
    // (Here the private address is not routed: the agent is sent to the panel directly,
    // as the link would carry it.)
    config.panel = addr.clone();
    config.save(&agent_file).unwrap();
    let agent = Agent::with_exec(&agent_file, config, Arc::new(Systemd), agent_fake.clone());
    tokio::spawn(agent.run());
    for _ in 0..100 {
        if listed(&hub).online {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let joined = listed(&hub);
    assert!(joined.online, "the agent came in as the waiting server");
    assert_eq!(joined.name, "behind-gre");
    // The agent made its end before it dialed: from its address to the panel's.
    let first = agent_fake.calls().into_iter().position(|c| {
        c.starts_with(&format!(
            "ip tunnel add {} mode gre local 192.0.2.2 remote 192.0.2.1",
            link.ifname
        ))
    });
    assert!(first.is_some(), "{:?}", agent_fake.calls());

    // ---- a code to reconnect it through the panel's private address carries the link ----
    let again = hub
        .create_rejoin(&made.id, &format!("{}:{port}", made.panel_addr), None)
        .unwrap();
    assert_eq!(kariz_panel::join::decode(&again).unwrap().g, Some(theirs));
    // One through a public address does not.
    let public = hub
        .create_rejoin(&made.id, "192.0.2.1:29001", None)
        .unwrap();
    assert_eq!(kariz_panel::join::decode(&public).unwrap().g, None);

    // ---- removing the server takes the link, and the panel's end of it, with it ----
    assert!(hub.remove(&made.id).unwrap());
    assert!(hub.networks.links(None).unwrap().is_empty());
    let mut gone = false;
    for _ in 0..50 {
        gone = local_fake
            .calls()
            .iter()
            .any(|c| c == &format!("ip link del {}", link.ifname));
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "{:?}", local_fake.calls());
}
