//! A real agent and a real hub over the panel's link, on the loopback: enrolling with a
//! join code, the code working once, reconnecting with the saved identity, and removal.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kariz_panel::agent::{self, Agent, AgentConfig};
use kariz_panel::db::Db;
use kariz_panel::hub::{Hub, ServerView, LOCAL};

/// A hub taking agents on a free loopback port; returns it with that address.
async fn start_hub(kariz_dir: &Path) -> (Arc<Hub>, String) {
    let hub = Hub::new(Db::in_memory().unwrap(), kariz_dir.to_path_buf());
    let acceptor = kariz::link::Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
        .await
        .unwrap();
    let addr = acceptor.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(acceptor));
    (hub, addr)
}

/// An agent enrolled with `code`, its settings in `dir`, its tunnel configs in `kariz_dir`.
fn agent_for(code: &str, dir: &Path, kariz_dir: &Path) -> (Arc<Agent>, std::path::PathBuf) {
    let path = dir.join("agent.toml");
    let mut config = agent::enroll_from_code(code, &path).unwrap();
    config.kariz_dir = kariz_dir.to_path_buf();
    config.save(&path).unwrap();
    (Agent::new(&path, config), path)
}

fn remote(hub: &Hub) -> Vec<ServerView> {
    hub.snapshot()
        .unwrap()
        .into_iter()
        .filter(|s| !s.local)
        .collect()
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..300 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

const TUNNEL: &str = "role = \"entry\"\nmode = \"reverse\"\n[tunnel]\ntransport = \"tcpmux\"\n\
                      listen = \"0.0.0.0:3080\"\ntoken = \"a-secret-token-0123456789\"\n\
                      [[forward]]\nlisten = \"0.0.0.0:443\"\ntarget = \"127.0.0.1:443\"\n";

#[tokio::test]
async fn a_server_joins_with_a_code_once_and_comes_back_with_its_identity() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kariz_panel=debug,kariz=warn")
        .with_test_writer()
        .try_init();
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let kariz_dir = dir.path().join("kariz");
    std::fs::create_dir_all(&kariz_dir).unwrap();
    std::fs::write(kariz_dir.join("main.toml"), TUNNEL).unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;

    // The panel makes a code, the agent joins with it.
    let code = hub.create_join(Some("istanbul-1"), &addr).unwrap();
    let (first, path) = agent_for(&code, dir.path(), &kariz_dir);
    let running = tokio::spawn(first.run());
    wait_for("the server to join", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;

    let servers = remote(&hub);
    assert_eq!(servers.len(), 1);
    let server = &servers[0];
    assert_eq!(server.name, "istanbul-1");
    assert_eq!(server.version, env!("CARGO_PKG_VERSION"));
    let id = server.id.clone();

    // It reports its tunnels (from its own directory), without the token.
    wait_for("the tunnel report", || remote(&hub)[0].tunnels.len() == 1).await;
    let view = &remote(&hub)[0].tunnels[0];
    assert_eq!(
        (view.info.name.as_str(), view.info.role.as_str()),
        ("main", "entry")
    );
    assert_eq!(view.info.forwards[0].listen, "0.0.0.0:443");
    assert!(!serde_json::to_string(&remote(&hub))
        .unwrap()
        .contains("a-secret-token"));

    // It kept an identity, and gave up the join secret.
    let saved = AgentConfig::load(&path).unwrap();
    assert_eq!(saved.id.as_deref(), Some(id.as_str()));
    assert!(saved.key.is_some() && saved.join.is_none());

    // The same code again (another server, or a stolen copy): nothing joins.
    let other = tempfile::tempdir().unwrap();
    let (second, _) = agent_for(&code, other.path(), &kariz_dir);
    let intruder = tokio::spawn(second.run());
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(remote(&hub).len(), 1, "a spent code must not add a server");
    intruder.abort();

    // The first agent goes away; the server shows as offline but stays known.
    running.abort();
    wait_for("the server to show offline", || !remote(&hub)[0].online).await;
    assert_eq!(remote(&hub)[0].id, id);

    // Restarted from its saved settings, it is the same server again.
    let restarted = Agent::new(&path, AgentConfig::load(&path).unwrap());
    let running = tokio::spawn(restarted.run());
    wait_for("the server to come back", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;
    assert_eq!(remote(&hub).len(), 1);
    assert_eq!(remote(&hub)[0].id, id);

    // Removing it drops it from the panel and closes its link.
    assert!(hub.remove(&id).unwrap());
    assert!(!hub.remove(&id).unwrap());
    assert!(remote(&hub).is_empty());
    assert!(
        !hub.remove("local").unwrap(),
        "the panel's own server cannot be removed"
    );
    running.abort();
}

#[tokio::test]
async fn an_agent_with_the_wrong_key_is_not_let_in_as_a_registered_server() {
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;
    let code = hub.create_join(None, &addr).unwrap();
    let (agent, path) = agent_for(&code, dir.path(), dir.path());
    let running = tokio::spawn(agent.run());
    wait_for("the server to join", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;
    let id = remote(&hub)[0].id.clone();
    running.abort();
    wait_for("offline", || !remote(&hub)[0].online).await;

    // Same id, a key the panel never made.
    let mut forged = AgentConfig::load(&path).unwrap();
    forged.key = Some("cd".repeat(32));
    let forged_agent = Agent::new(&path, forged);
    let attempt = tokio::spawn(forged_agent.run());
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        !remote(&hub)[0].online,
        "a forged key must not bring {id} online"
    );
    attempt.abort();
}

#[test]
fn join_codes_carry_the_link_token_and_a_secret_that_is_not_stored_as_it_is() {
    let hub = Hub::new(
        Db::in_memory().unwrap(),
        std::path::PathBuf::from("/nonexistent"),
    );
    let code = kariz_panel::join::decode(&hub.create_join(Some("a"), "203.0.113.5:29001").unwrap())
        .unwrap();
    assert_eq!(code.p, "203.0.113.5:29001");
    assert_eq!(code.t, hub.link_token().unwrap());
    assert_eq!(code.n.as_deref(), Some("a"));
    assert_eq!(code.j.len(), 32);
}

/// One link of a hand-driven agent: answers Hello with `hello`, then answers Enroll (or,
/// with `drop_enroll`, receives it and drops the link without an answer, as a path that
/// stalls does). Returns the identity the panel sent.
async fn enroll_once(
    addr: &str,
    token: &str,
    hello: impl Fn(&str) -> kariz_panel::wire::HelloReply,
    drop_enroll: bool,
) -> Option<(String, String)> {
    use bytes::Bytes;
    use kariz_panel::wire::{Ack, Request};
    let dialer = kariz::link::Dialer::new(addr, token).unwrap();
    let session = dialer.connect(kariz::mux::Side::Server).await.unwrap();
    let mut got = None;
    while let Ok(Some((stream, syn))) =
        tokio::time::timeout(Duration::from_secs(5), session.accept()).await
    {
        let answer = match serde_json::from_slice::<Request>(&syn).unwrap() {
            Request::Hello { challenge } => serde_json::to_vec(&hello(&challenge)).unwrap(),
            Request::Enroll { id, key } => {
                got = Some((id, key));
                if drop_enroll {
                    session.close();
                    return got;
                }
                serde_json::to_vec(&Ack {
                    ok: true,
                    error: None,
                })
                .unwrap()
            }
            _ => break,
        };
        stream.send(Bytes::from(answer)).await.unwrap();
        stream.finish().unwrap();
        if got.is_some() {
            // let the answer go out before the link is dropped
            tokio::time::sleep(Duration::from_millis(300)).await;
            break;
        }
    }
    session.close();
    got
}

#[tokio::test]
async fn an_enrollment_cut_short_is_finished_later_with_the_same_identity() {
    use kariz_panel::wire::HelloReply;
    let panel_dir = tempfile::tempdir().unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;
    let token = hub.link_token().unwrap();
    let code = kariz_panel::join::decode(&hub.create_join(Some("cut"), &addr).unwrap()).unwrap();
    let new_agent = |_: &str| HelloReply {
        id: None,
        proof: None,
        join: Some(code.j.clone()),
        hostname: "h".into(),
        version: "1".into(),
        arch: "x".into(),
    };

    // The Enroll arrives, but its answer never gets back.
    let (id, key) = enroll_once(&addr, &token, new_agent, true)
        .await
        .expect("an Enroll");
    wait_for("the server to be kept", || {
        remote(&hub).iter().any(|s| s.id == id && !s.online)
    })
    .await;

    // An agent that did not keep it: the same code gets the same identity again.
    let again = enroll_once(&addr, &token, new_agent, false)
        .await
        .expect("an Enroll");
    assert_eq!(
        again,
        (id.clone(), key.clone()),
        "the same identity, not a new server"
    );

    // An agent that did keep it proves it and is let in.
    let (pid, pkey) = (id.clone(), key.clone());
    let proven = move |challenge: &str| HelloReply {
        id: Some(pid.clone()),
        proof: kariz_panel::agent::proof(&pkey, challenge),
        join: None,
        hostname: "h".into(),
        version: "1".into(),
        arch: "x".into(),
    };
    tokio::spawn({
        let (addr, token) = (addr.clone(), token.clone());
        async move { enroll_once(&addr, &token, proven, false).await }
    });
    wait_for("the server to come online", || {
        remote(&hub).iter().any(|s| s.id == id && s.online)
    })
    .await;
    assert_eq!(remote(&hub).len(), 1);

    // Confirmed: the code gives nothing out any more.
    assert_eq!(enroll_once(&addr, &token, new_agent, false).await, None);
}

#[tokio::test]
async fn a_code_can_pin_the_agent_to_wss() {
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), panel_dir.path().to_path_buf());
    let token = hub.link_token().unwrap();
    // A certificate for another name: the agent does not check it (the token does).
    let c = rcgen::generate_simple_self_signed(vec!["panel.example".to_owned()]).unwrap();
    let (cert, key) = (
        panel_dir.path().join("c.pem"),
        panel_dir.path().join("k.pem"),
    );
    std::fs::write(&cert, c.cert.pem()).unwrap();
    std::fs::write(&key, c.signing_key.serialize_pem()).unwrap();
    // tcpmux on the agents port, wss on the next one, as a panel listens.
    let (tcp, wss) = loop {
        let tcp = kariz::link::Acceptor::bind("127.0.0.1:0", &token)
            .await
            .unwrap();
        let next = kariz::link::wss_addr(&tcp.local_addr().unwrap().to_string()).unwrap();
        if let Ok(wss) = kariz::link::Acceptor::bind_wss(&next, &token, &cert, &key).await {
            break (tcp, wss);
        }
    };
    let addr = tcp.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(tcp));
    tokio::spawn(hub.clone().serve_agents(wss));

    let code = hub
        .create_join_via(Some("pinned"), &addr, Some("wss"))
        .unwrap();
    assert!(hub
        .create_join_via(None, &addr, Some("carrier-pigeon"))
        .is_err());
    let (agent, path) = agent_for(&code, dir.path(), dir.path());
    assert_eq!(
        AgentConfig::load(&path).unwrap().transport.as_deref(),
        Some("wss")
    );
    let running = tokio::spawn(agent.run());
    wait_for("the server to join over wss", || {
        remote(&hub)
            .iter()
            .any(|s| s.online && s.link.as_deref() == Some("wss"))
    })
    .await;
    running.abort();
}

#[tokio::test]
async fn a_tunnel_deleted_while_its_server_is_away_goes_when_the_server_is_back() {
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let kariz_dir = dir.path().join("kariz");
    std::fs::create_dir_all(&kariz_dir).unwrap();
    std::fs::write(kariz_dir.join("main.toml"), TUNNEL).unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;

    let code = hub.create_join(Some("away"), &addr).unwrap();
    let (agent, path) = agent_for(&code, dir.path(), &kariz_dir);
    let running = tokio::spawn(agent.run());
    wait_for("the server to join", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;
    wait_for("its tunnel", || remote(&hub)[0].tunnels.len() == 1).await;
    let id = remote(&hub)[0].id.clone();

    // It goes away; the tunnel is deleted from the panel meanwhile.
    running.abort();
    wait_for("the server to show offline", || !remote(&hub)[0].online).await;
    let op = kariz_panel::pair::delete(&hub, "main").unwrap();
    wait_for("the delete to finish", || {
        hub.ops.get(&op).is_some_and(|o| o.state != "running")
    })
    .await;
    let done = hub.ops.get(&op).unwrap();
    assert!(done.state == "done" && done.error.is_none(), "{done:?}");
    assert_eq!(done.offline, vec![id.clone()]);
    // It is gone from the lists at once, its file is still there, and the name is kept.
    assert!(remote(&hub)[0].tunnels.is_empty());
    assert!(kariz_dir.join("main.toml").exists());
    assert!(hub.delete_pending_for("main"));

    // When the agent is back, it removes the tunnel and the panel forgets the delete.
    let back = Agent::new(&path, AgentConfig::load(&path).unwrap());
    let back = tokio::spawn(back.run());
    wait_for("the file to go", || !kariz_dir.join("main.toml").exists()).await;
    wait_for("the delete to be forgotten", || {
        !hub.delete_pending_for("main")
    })
    .await;
    back.abort();
}

#[tokio::test]
async fn a_lost_server_is_taken_over_by_a_new_agent_and_keeps_its_name_and_tunnels() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("kariz_panel=debug,kariz=warn")
        .with_test_writer()
        .try_init();
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let kariz_dir = dir.path().join("kariz");
    std::fs::create_dir_all(&kariz_dir).unwrap();
    std::fs::write(kariz_dir.join("main.toml"), TUNNEL).unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;

    let code = hub.create_join(Some("istanbul-1"), &addr).unwrap();
    let (first, old_path) = agent_for(&code, dir.path(), &kariz_dir);
    let running = tokio::spawn(first.run());
    wait_for("the server to join", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;
    wait_for("its tunnel", || remote(&hub)[0].tunnels.len() == 1).await;
    let id = remote(&hub)[0].id.clone();

    // It goes away. The panel still knows it, when it was last heard, and why the link ended.
    running.abort();
    wait_for("the server to show offline", || !remote(&hub)[0].online).await;
    assert!(remote(&hub)[0].last_seen.is_some());

    // A code for this server, with another link transport; there is none for the panel's own
    // server or for one that is not known.
    assert!(hub.create_rejoin("nobody", &addr, None).is_err());
    assert!(hub.create_rejoin(LOCAL, &addr, None).is_err());
    assert!(hub.create_rejoin(&id, &addr, Some("nonsense")).is_err());
    let fresh = hub.create_rejoin(&id, &addr, Some("tcpmux")).unwrap();
    assert_eq!(
        kariz_panel::join::decode(&fresh).unwrap().x.as_deref(),
        Some("tcpmux")
    );

    // A new agent (a reinstalled server, with the same tunnel files) joins with it: the same
    // server, not a second one, with its name and its tunnel.
    let other = tempfile::tempdir().unwrap();
    let (second, new_path) = agent_for(&fresh, other.path(), &kariz_dir);
    let running = tokio::spawn(second.run());
    wait_for("the server to come back", || {
        remote(&hub).iter().any(|s| s.online)
    })
    .await;
    let servers = remote(&hub);
    assert_eq!(
        servers.len(),
        1,
        "a re-joined server is not a second server"
    );
    assert_eq!(
        (servers[0].id.as_str(), servers[0].name.as_str()),
        (id.as_str(), "istanbul-1")
    );
    wait_for("its tunnel", || remote(&hub)[0].tunnels.len() == 1).await;
    assert!(servers[0].last_error.is_none());
    let saved = AgentConfig::load(&new_path).unwrap();
    assert_eq!(saved.id.as_deref(), Some(id.as_str()));
    assert!(saved.join.is_none());

    // The code worked once.
    let third = tempfile::tempdir().unwrap();
    let (late, _) = agent_for(&fresh, third.path(), &kariz_dir);
    let intruder = tokio::spawn(late.run());
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(remote(&hub).len(), 1);
    intruder.abort();

    // The old agent's key no longer opens the server, and the panel says so.
    running.abort();
    wait_for("offline again", || !remote(&hub)[0].online).await;
    let old = Agent::new(&old_path, AgentConfig::load(&old_path).unwrap());
    let stale = tokio::spawn(old.run());
    wait_for("the refusal to be noted", || {
        remote(&hub)[0]
            .last_error
            .as_ref()
            .is_some_and(|e| e.kind == "wrong_key")
    })
    .await;
    assert!(!remote(&hub)[0].online);
    stale.abort();

    // The new agent's saved identity brings it back.
    let back = Agent::new(&new_path, AgentConfig::load(&new_path).unwrap());
    let back = tokio::spawn(back.run());
    wait_for("the new agent to connect", || remote(&hub)[0].online).await;
    back.abort();
}

#[tokio::test]
async fn a_server_can_be_renamed_but_not_to_a_name_that_is_taken() {
    let panel_dir = tempfile::tempdir().unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let (a, _) = agent_for(
        &hub.create_join(Some("one"), &addr).unwrap(),
        dir_a.path(),
        dir_a.path(),
    );
    let a = tokio::spawn(a.run());
    wait_for("the first server", || {
        remote(&hub).len() == 1 && remote(&hub)[0].online
    })
    .await;
    let (b, _) = agent_for(
        &hub.create_join(Some("two"), &addr).unwrap(),
        dir_b.path(),
        dir_b.path(),
    );
    let b = tokio::spawn(b.run());
    wait_for("the second server", || remote(&hub).len() == 2).await;
    let ids: Vec<String> = remote(&hub).iter().map(|s| s.id.clone()).collect();

    hub.rename(&ids[0], "frankfurt").unwrap();
    assert_eq!(remote(&hub)[0].name, "frankfurt");
    assert_eq!(
        hub.rename(&ids[1], "frankfurt").unwrap_err().to_string(),
        "name_taken"
    );
    assert_eq!(
        hub.rename(&ids[0], "Bad Name!").unwrap_err().to_string(),
        "bad_name"
    );
    assert!(hub.rename(LOCAL, "mine").is_err());
    // Its own name again is fine.
    hub.rename(&ids[0], "frankfurt").unwrap();
    a.abort();
    b.abort();
}

/// Services that only write down what they were asked, and say which tunnels are stopped.
struct Recorder {
    calls: std::sync::Mutex<Vec<(String, String)>>,
    stopped: Vec<&'static str>,
}

impl kariz_panel::manage::Services for Recorder {
    fn ctl(&self, name: String, action: String) -> kariz_panel::manage::Fut<anyhow::Result<()>> {
        self.calls.lock().unwrap().push((name, action));
        Box::pin(async { Ok(()) })
    }
    fn active(&self, name: String) -> kariz_panel::manage::Fut<Option<bool>> {
        let stopped = self.stopped.contains(&name.as_str());
        Box::pin(async move { Some(!stopped) })
    }
}

fn result_of(hub: &Hub, id: &str) -> (i64, String) {
    let s = hub
        .schedules()
        .unwrap()
        .into_iter()
        .find(|s| s.subject == id)
        .unwrap();
    (s.last_run, s.last_result)
}

#[tokio::test]
async fn a_server_restart_timer_restarts_what_runs_and_waits_for_a_server_that_is_away() {
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let kariz_dir = dir.path().join("kariz");
    std::fs::create_dir_all(&kariz_dir).unwrap();
    std::fs::write(kariz_dir.join("main.toml"), TUNNEL).unwrap();
    std::fs::write(kariz_dir.join("quiet.toml"), TUNNEL).unwrap();
    let (hub, addr) = start_hub(panel_dir.path()).await;

    let code = hub.create_join(Some("far"), &addr).unwrap();
    let path = dir.path().join("agent.toml");
    let mut config = agent::enroll_from_code(&code, &path).unwrap();
    config.kariz_dir = kariz_dir.clone();
    config.save(&path).unwrap();
    let recorder = Arc::new(Recorder {
        calls: Default::default(),
        stopped: vec!["quiet"],
    });
    let running = tokio::spawn(Agent::with_services(&path, config, recorder.clone()).run());
    wait_for("the server and its tunnels", || {
        remote(&hub)
            .iter()
            .any(|s| s.online && s.tunnels.len() == 2)
    })
    .await;
    let id = remote(&hub)[0].id.clone();

    // What is not allowed.
    use kariz_panel::schedule::{self, NewSchedule};
    let timer = |subject: &str, mode: &str, every: i64, daily: i64| NewSchedule {
        kind: "server".into(),
        subject: subject.into(),
        mode: mode.into(),
        every_secs: every,
        daily_min: daily,
        enabled: true,
    };
    assert!(
        schedule::set(&hub, &timer(&id, "every", 5, 0)).is_err(),
        "under ten minutes"
    );
    assert!(schedule::set(&hub, &timer(&id, "daily", 0, 1440)).is_err());
    assert!(schedule::set(&hub, &timer("nobody", "every", 3600, 0)).is_err());
    let bad_tunnel = NewSchedule {
        kind: "tunnel".into(),
        subject: "nothing".into(),
        ..timer(&id, "every", 3600, 0)
    };
    assert!(schedule::set(&hub, &bad_tunnel).is_err());

    // A timer every hour: counted from now, so not due yet.
    schedule::set(&hub, &timer(&id, "every", 3600, 0)).unwrap();
    let s = hub.schedules().unwrap().remove(0);
    assert_eq!((s.last_run, s.last_result.as_str()), (0, ""));
    assert!(s.next_run.unwrap() > kariz_panel::auth::now() + 3000);

    // Run now: the tunnel that runs is restarted, the stopped one is left alone.
    assert!(schedule::run_now(&hub, "server", &id));
    wait_for("the restart", || !result_of(&hub, &id).1.is_empty()).await;
    assert_eq!(result_of(&hub, &id).1, "ok:1");
    assert!(result_of(&hub, &id).0 > 0);
    assert_eq!(
        *recorder.calls.lock().unwrap(),
        vec![("main".to_owned(), "restart".to_owned())]
    );
    assert!(hub
        .history
        .events(10)
        .unwrap()
        .iter()
        .any(|e| e.kind == "auto_restart_server" && e.subject == id));

    // The server goes away: nothing is restarted, the turn is kept for when it is back.
    running.abort();
    wait_for("offline", || !remote(&hub)[0].online).await;
    let before = result_of(&hub, &id).0;
    assert!(schedule::run_now(&hub, "server", &id));
    wait_for("the skip", || result_of(&hub, &id).1 == "skipped_offline").await;
    assert_eq!(result_of(&hub, &id).0, before);
    assert_eq!(recorder.calls.lock().unwrap().len(), 1);

    // Removing the server removes its timer.
    hub.remove(&id).unwrap();
    assert!(hub.schedules().unwrap().is_empty());
}
