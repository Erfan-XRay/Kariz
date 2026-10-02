//! An agent that joins over `quic`: the link is QUIC, always sealed with the token's key
//! (nothing to switch on), so a network that filters QUIC does not see it.

use std::time::Duration;

use kariz_panel::agent::{self, Agent};
use kariz_panel::db::Db;
use kariz_panel::hub::Hub;

#[tokio::test]
async fn an_agent_joins_over_quic_and_it_is_remembered() {
    let panel_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), panel_dir.path().to_path_buf());
    let token = hub.link_token().unwrap();

    // The panel's QUIC listener is on the port after its agents port: a code names the
    // agents port, and the agent adds one for quic, as for wss.
    let quic =
        kariz::link::Acceptor::bind_via("127.0.0.1:0", &token, kariz::config::TransportKind::Quic)
            .await
            .unwrap();
    let port = quic.local_addr().unwrap().port();
    tokio::spawn(hub.clone().serve_agents(quic));

    let code = hub
        .create_join_via(
            Some("behind-a-filter"),
            &format!("127.0.0.1:{}", port - 1),
            Some("quic"),
        )
        .unwrap();
    let path = dir.path().join("agent.toml");
    let mut config = agent::enroll_from_code(&code, &path).unwrap();
    assert_eq!(config.transport.as_deref(), Some("quic"));
    config.kariz_dir = dir.path().join("kariz");
    config.save(&path).unwrap();
    let running = tokio::spawn(Agent::new(&path, config).run());

    let mut ok = false;
    for _ in 0..300 {
        let servers = hub.snapshot().unwrap();
        if servers
            .iter()
            .any(|s| !s.local && s.online && s.health.is_some())
        {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    running.abort();
    assert!(ok, "the server never came online over quic");
    let servers = hub.snapshot().unwrap();
    let remote = servers.iter().find(|s| !s.local).unwrap();
    assert_eq!(remote.link.as_deref(), Some("quic"));
}
