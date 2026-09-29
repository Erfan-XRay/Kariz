//! Finding, downloading and checking a release, against a small local server that answers
//! the way GitHub does (docs/PHASE14.md). The swap itself is tested in `update.rs` with a
//! fake host, and for real by CI on a systemd host.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use kariz_panel::db::Db;
use kariz_panel::hub::Hub;
use kariz_panel::sign;
use kariz_panel::update::{self, arch_name};
use kariz_panel::updater::{self, UpdateSettings};

/// Serves `files` (path to bytes) on a free loopback port; returns its address.
fn serve(files: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let files = files.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let reply = match files.get(&path) {
                    Some(body) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = stream.write_all(&reply);
            });
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

/// A change to the served files, to break a release in one particular way.
type Tweak = Box<dyn FnOnce(&mut HashMap<String, Vec<u8>>)>;

fn archive() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in [
        ("kariz-v99.0.0/kariz", &b"the core"[..]),
        ("kariz-v99.0.0/kariz-panel", &b"the panel"[..]),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
    }
    let tar = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

/// A repository with one release, `tag`, signed by a fresh key (whose public half comes
/// back), with the parts a test wants to break: `tweak` edits the files before serving.
fn repo(tag: &str, tweak: impl FnOnce(&mut HashMap<String, Vec<u8>>)) -> (String, String) {
    let arch = arch_name().expect("a CPU with releases");
    let names = update::asset_names(tag, arch);
    let (private, public) = sign::generate().unwrap();
    let archive = archive();
    let sha = format!("{}  {}\n", sign::sha256_hex(&archive), names[0]).into_bytes();
    let sig = sign::sign(&private, &sha).unwrap();
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    files.insert(format!("/dl/{}", names[0]), archive);
    files.insert(format!("/dl/{}", names[1]), sha);
    files.insert(format!("/dl/{}", names[2]), sig);
    tweak(&mut files);
    // the URLs need the server's port, which is only known once it runs: serve the
    // downloads first, then the listing that points at them
    let base = serve(files.clone());
    let assets: Vec<String> = names
        .iter()
        .map(|n| format!(r#"{{"name":"{n}","browser_download_url":"{base}/dl/{n}"}}"#))
        .collect();
    let listing = format!(
        r#"[{{"tag_name":"{tag}","body":"what is new","prerelease":false,"draft":false,"assets":[{}]}}]"#,
        assets.join(",")
    );
    let api = serve(HashMap::from([(
        "/repos/x/y/releases?per_page=20".to_owned(),
        listing.into_bytes(),
    )]));
    (format!("{api}/repos/x/y"), sign::hex(&public))
}

fn hub_for(api: &str, key: &str, dir: &std::path::Path) -> Arc<Hub> {
    let hub = Hub::new(Db::in_memory().unwrap(), dir.join("kariz"));
    hub.set_update_settings(UpdateSettings {
        api: api.to_owned(),
        key: Some(key.to_owned()),
        dir: dir.join("updates"),
        config: dir.join("panel.toml"),
        panel_bin: dir.join("bin/kariz-panel"),
        kariz_bin: dir.join("bin/kariz"),
    });
    hub
}

#[tokio::test]
async fn a_newer_release_is_found_downloaded_verified_and_staged() {
    let dir = tempfile::tempdir().unwrap();
    let (api, key) = repo("v99.0.0", |_| {});
    let hub = hub_for(&api, &key, dir.path());

    let before = hub.update_status();
    assert_eq!(before["state"], "none");
    assert_eq!(before["current"], kariz_panel::version());
    hub.check_update().await.unwrap();
    let status = hub.update_status();
    assert_eq!(status["state"], "newer", "{status}");
    assert_eq!(status["latest"]["version"], "99.0.0");
    assert_eq!(status["latest"]["notes"], "what is new");
    assert_eq!(status["major"], true, "a new major version is marked");
    assert!(status["error"].is_null());

    // a new major version needs the user's word
    let refused = updater::start(&hub, false).unwrap_err();
    assert_eq!(format!("{refused:#}"), "major_needs_confirm");

    // downloading verifies and stages both programs, and keeps the release for the agents
    let release = hub.checked.lock().unwrap().release.clone().unwrap();
    let steps = std::sync::Mutex::new(Vec::new());
    let staged = update::download(&release, Some(&key), &dir.path().join("updates"), &|s| {
        steps.lock().unwrap().push(s.to_owned())
    })
    .unwrap();
    assert_eq!(
        *steps.lock().unwrap(),
        ["upd_download", "upd_verify", "upd_stage"]
    );
    assert_eq!(
        std::fs::read(staged.join("kariz-panel")).unwrap(),
        b"the panel"
    );
    assert_eq!(std::fs::read(staged.join("kariz")).unwrap(), b"the core");
    let arch = arch_name().unwrap();
    let names = update::asset_names("v99.0.0", arch);
    for n in &names {
        assert!(staged.join("release").join(n).is_file(), "{n} is kept");
    }
}

#[tokio::test]
async fn a_release_that_is_not_the_one_the_key_signed_is_refused_and_nothing_is_staged() {
    let code = |r: anyhow::Result<std::path::PathBuf>| format!("{:#}", r.unwrap_err());
    let arch = arch_name().unwrap();
    let names = update::asset_names("v99.0.0", arch);
    let tries: Vec<(&str, Tweak, &str)> = vec![
        (
            "a changed archive",
            Box::new({
                let n = names[0].clone();
                move |f| {
                    f.insert(format!("/dl/{n}"), b"not the real archive".to_vec());
                }
            }),
            "bad_checksum",
        ),
        (
            "a damaged signature",
            Box::new({
                let n = names[2].clone();
                move |f| {
                    f.get_mut(&format!("/dl/{n}")).unwrap()[3] ^= 1;
                }
            }),
            "bad_signature",
        ),
        (
            "no signature",
            Box::new({
                let n = names[2].clone();
                move |f| {
                    f.remove(&format!("/dl/{n}"));
                }
            }),
            "unsigned",
        ),
    ];
    for (what, tweak, want) in tries {
        let dir = tempfile::tempdir().unwrap();
        let (api, key) = repo("v99.0.0", tweak);
        let hub = hub_for(&api, &key, dir.path());
        hub.check_update().await.unwrap();
        let release = hub.checked.lock().unwrap().release.clone().unwrap();
        let got = code(update::download(
            &release,
            Some(&key),
            &dir.path().join("updates"),
            &|_| {},
        ));
        assert!(got.contains(want), "{what}: {got}");
        assert!(
            !dir.path().join("updates/99.0.0").exists(),
            "{what}: nothing staged"
        );
    }

    // signed by another key than the one the panel trusts
    let dir = tempfile::tempdir().unwrap();
    let (api, _key) = repo("v99.0.0", |_| {});
    let (_, other) = sign::generate().unwrap();
    let hub = hub_for(&api, &sign::hex(&other), dir.path());
    hub.check_update().await.unwrap();
    let release = hub.checked.lock().unwrap().release.clone().unwrap();
    let got = code(update::download(
        &release,
        Some(&sign::hex(&other)),
        &dir.path().join("updates"),
        &|_| {},
    ));
    assert!(got.contains("bad_signature"), "{got}");
}

#[tokio::test]
async fn the_same_or_an_older_release_is_not_an_update_and_a_dead_repository_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (api, key) = repo(&format!("v{}", kariz_panel::version()), |_| {});
    let hub = hub_for(&api, &key, dir.path());
    hub.check_update().await.unwrap();
    assert_eq!(hub.update_status()["state"], "same");
    assert_eq!(
        format!("{:#}", updater::start(&hub, true).unwrap_err()),
        "no_update"
    );

    let (api, key) = repo("v0.0.1", |_| {});
    let hub = hub_for(&api, &key, dir.path());
    hub.check_update().await.unwrap();
    assert_eq!(hub.update_status()["state"], "older", "no downgrades");
    assert_eq!(
        format!("{:#}", updater::start(&hub, true).unwrap_err()),
        "no_update"
    );

    // a repository that cannot be reached: the status says so, the panel goes on
    let hub = hub_for("http://127.0.0.1:1/repos/x/y", &key, dir.path());
    assert!(hub.check_update().await.is_err());
    let status = hub.update_status();
    assert!(
        status["error"]
            .as_str()
            .unwrap()
            .contains("download_failed"),
        "{status}"
    );
    assert_eq!(status["state"], "none");
}

#[tokio::test]
async fn the_channel_and_the_daily_check_are_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (api, key) = repo("v99.0.0", |_| {});
    let hub = hub_for(&api, &key, dir.path());
    assert_eq!(hub.update_status()["channel"], "stable");
    assert_eq!(hub.update_status()["auto"], true);
    hub.check_update().await.unwrap();
    assert_eq!(hub.update_status()["state"], "newer");
    // changing the channel forgets what was found for the other one
    hub.set_update_options(Some(update::Channel::Beta), Some(false))
        .unwrap();
    let s = hub.update_status();
    assert_eq!(
        (
            s["channel"].as_str(),
            s["auto"].as_bool(),
            s["state"].as_str()
        ),
        (Some("beta"), Some(false), Some("none"))
    );
}
