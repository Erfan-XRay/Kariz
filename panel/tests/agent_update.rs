//! A release sent to a real agent over the panel's link, the way the rolling update does it:
//! begin, many pieces at once, apply. The helper is recorded instead of started.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kariz_panel::agent::{self, Agent};
use kariz_panel::agent_update::Launcher;
use kariz_panel::db::Db;
use kariz_panel::hub::Hub;
use kariz_panel::manage::Systemd;
use kariz_panel::net::Real;
use kariz_panel::{sign, update, updater};

type Launches = Arc<Mutex<Vec<(PathBuf, Vec<String>)>>>;

struct Recorder(Launches);

impl Launcher for Recorder {
    fn launch(&self, program: &Path, args: Vec<String>) -> anyhow::Result<()> {
        self.0.lock().unwrap().push((program.to_path_buf(), args));
        Ok(())
    }
}

/// A release whose archive is big enough to need hundreds of pieces.
fn release(version: &str) -> (updater::Files, String) {
    let arch = update::arch_name().unwrap();
    let names = update::asset_names(&format!("v{version}"), arch);
    let mut builder = tar::Builder::new(Vec::new());
    // Not compressible, so the archive really is a few megabytes.
    let mut noise = vec![0u8; 3 * 1024 * 1024];
    getrandom::fill(&mut noise).unwrap();
    for (name, data) in [
        ("kariz-x/kariz", &noise[..]),
        ("kariz-x/kariz-panel", &b"the new panel"[..]),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    let archive = gz.finish().unwrap();
    let (private, public) = sign::generate().unwrap();
    let sha = format!("{}  {}\n", sign::sha256_hex(&archive), names[0]).into_bytes();
    let sig = sign::sign(&private, &sha).unwrap();
    (
        vec![
            (names[0].clone(), archive),
            (names[1].clone(), sha),
            (names[2].clone(), sig),
        ],
        sign::hex(&public),
    )
}

async fn join(hub: &Arc<Hub>, root: &Path, key: &str, launches: Launches) -> String {
    let acceptor = kariz::link::Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
        .await
        .unwrap();
    let addr = acceptor.local_addr().unwrap().to_string();
    tokio::spawn(hub.clone().serve_agents(acceptor));
    let code = hub.create_join(Some("far-away"), &addr).unwrap();
    let file = root.join("agent").join("agent.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut config = agent::enroll_from_code(&code, &file).unwrap();
    config.release_key = Some(key.to_owned());
    config.save(&file).unwrap();
    let agent = Agent::with_launcher(
        &file,
        config,
        Arc::new(Systemd),
        Arc::new(Real),
        Box::new(Recorder(launches)),
    );
    tokio::spawn(agent.run());
    for _ in 0..100 {
        if let Some(s) = hub
            .snapshot()
            .unwrap()
            .into_iter()
            .find(|s| !s.local && s.online)
        {
            return s.id;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the agent did not join");
}

#[tokio::test]
async fn a_release_is_sent_in_pieces_checked_by_the_agent_and_handed_to_the_helper() {
    let root = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), root.path().join("kariz"));
    let (files, key) = release("99.0.0");
    let launches: Launches = Launches::default();
    let id = join(&hub, root.path(), &key, launches.clone()).await;

    updater::send_release(&hub, &id, "99.0.0", &files)
        .await
        .unwrap();
    let launched = launches.lock().unwrap().clone();
    assert_eq!(launched.len(), 1, "the helper was started once");
    assert_eq!(launched[0].1[0], "agent-update-apply");
    let stage = root.path().join("agent/updates/99.0.0");
    assert_eq!(
        std::fs::read(stage.join("kariz-panel")).unwrap(),
        b"the new panel"
    );
    assert_eq!(
        std::fs::metadata(stage.join("kariz")).unwrap().len(),
        3 * 1024 * 1024
    );
}

#[tokio::test]
async fn a_release_signed_by_a_key_the_agent_does_not_trust_is_refused_over_the_link() {
    let root = tempfile::tempdir().unwrap();
    let hub = Hub::new(Db::in_memory().unwrap(), root.path().join("kariz"));
    let (files, _) = release("99.0.0");
    let (_, other) = release("99.0.0");
    let launches: Launches = Launches::default();
    let id = join(&hub, root.path(), &other, launches.clone()).await;

    let err = updater::send_release(&hub, &id, "99.0.0", &files)
        .await
        .unwrap_err();
    assert_eq!(format!("{err:#}"), "bad_signature");
    assert!(launches.lock().unwrap().is_empty(), "nothing was started");
    assert!(!root.path().join("agent/updates/99.0.0").exists());
}
