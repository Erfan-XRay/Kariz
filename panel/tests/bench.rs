//! The benchmark between the panel's own server and a real agent, on this machine: every
//! transport in both directions is probed, the best few are measured, scored and kept; a
//! second run of the same pair waits for the first, and a run can be stopped.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kariz_panel::agent::{self, Agent};
use kariz_panel::bench::{self, Ask, Bench};
use kariz_panel::db::Db;
use kariz_panel::hub::{Hub, LOCAL};
use kariz_panel::manage::Systemd;

async fn ended(hub: &Hub, id: &str) -> Bench {
    for _ in 0..1200 {
        let b = hub.bench.get(id).expect("the run is known");
        if !matches!(b.state.as_str(), "probe" | "speed") {
            return b;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the benchmark did not end");
}

#[tokio::test]
async fn two_servers_are_benchmarked_scored_and_the_result_kept() {
    let root = tempfile::tempdir().unwrap();
    let hub = Hub::with_options(
        Db::in_memory().unwrap(),
        root.path().join("kariz"),
        Arc::new(Systemd),
        Duration::from_secs(5),
        Some(root.path().join("panel")),
        None,
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
    tokio::spawn(Agent::new(&agent_file, config).run());
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
    let ask = |profile: &str| Ask {
        entry: LOCAL.into(),
        exit: remote.clone(),
        profile: profile.into(),
        port: 0,
        modes: Vec::new(),
        hosts: HashMap::from([
            (LOCAL.to_owned(), "127.0.0.1".to_owned()),
            (remote.clone(), "127.0.0.1".to_owned()),
        ]),
    };

    let why = |r: anyhow::Result<String>| format!("{:#}", r.unwrap_err());
    let mut same = ask("balanced");
    same.exit = LOCAL.into();
    assert_eq!(why(bench::start(&hub, same)), "same_server");
    assert_eq!(why(bench::start(&hub, ask("fastest"))), "bad_input");
    assert!(bench::last(&hub, LOCAL, &remote).unwrap().is_none());

    let started = std::time::Instant::now();
    let id = bench::start(&hub, ask("gaming")).unwrap();
    // The same two servers wait for the run that is going.
    assert_eq!(why(bench::start(&hub, ask("gaming"))), "busy");
    let done = ended(&hub, &id).await;
    let took = started.elapsed();
    assert_eq!(done.state, "done", "{done:#?}");
    assert!(took < Duration::from_secs(45), "it took {took:?}");

    // Every transport, both directions; on one machine all of them get through.
    assert!(done.candidates.len() >= 8, "{:#?}", done.candidates);
    let ok: Vec<_> = done.candidates.iter().filter(|c| c.state == "ok").collect();
    for transport in ["tcpmux", "ws", "wss"] {
        assert!(
            ok.iter().any(|c| c.transport == transport),
            "{transport} did not get through: {:#?}",
            done.candidates
        );
    }
    for c in &ok {
        assert!(c.latency.as_ref().unwrap().received > 0, "{c:#?}");
    }
    // The best few were measured for speed, and only they are ranked; the best is one of
    // them.
    let measured: Vec<_> = ok.iter().filter(|c| c.download_mbps.is_some()).collect();
    assert!((1..=3).contains(&measured.len()), "{measured:#?}");
    for c in &ok {
        assert_eq!(c.score.is_some(), c.download_mbps.is_some(), "{c:#?}");
        assert!(c.score.unwrap_or(0) <= 100);
    }
    let best = done.best.clone().expect("a best one");
    assert!(measured.iter().any(|c| c.key == best));
    assert!(done.candidates.iter().any(|c| c.mode == "reverse"));
    assert!(done.candidates.iter().any(|c| c.mode == "direct"));

    // Kept as the last result of the pair, in that order.
    let kept = bench::last(&hub, LOCAL, &remote).unwrap().expect("kept");
    assert_eq!(kept.best, done.best);
    assert!(bench::last(&hub, &remote, LOCAL).unwrap().is_none());

    // A run that is stopped ends at its next step, with what it had.
    let id = bench::start(&hub, ask("balanced")).unwrap();
    assert!(hub.bench.stop(&id));
    let stopped = ended(&hub, &id).await;
    assert_eq!(stopped.state, "stopped");
    assert!(!hub.bench.stop(&id), "it is not running any more");
}
