//! The panel's part of updating (docs/PHASE14.md, sections 2 and 4): remember what the last
//! check found, answer the browser, and run an update as an operation whose steps the
//! browser follows. The downloading, verifying and swapping themselves are in `update.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::auth::now;
use crate::hub::Hub;
use crate::update::{self, Channel, Release, Step};

/// What the panel needs to update itself: where the releases are, which key checks them,
/// and where its own files are.
#[derive(Debug, Clone)]
pub struct UpdateSettings {
    /// `https://api.github.com/repos/OWNER/REPO`.
    pub api: String,
    /// A pinned release key (hex), instead of the built-in one.
    pub key: Option<String>,
    /// Where downloads are kept (`/var/lib/kariz-panel/updates`).
    pub dir: PathBuf,
    /// `panel.toml`, which the swap helper reads to find the panel again.
    pub config: PathBuf,
    /// The installed programs to replace.
    pub panel_bin: PathBuf,
    pub kariz_bin: PathBuf,
}

#[derive(Default)]
pub struct Checked {
    pub release: Option<Release>,
    pub at: Option<i64>,
    pub error: Option<String>,
}

impl Hub {
    pub fn set_update_settings(&self, settings: UpdateSettings) {
        let _ = self.update_settings.set(settings);
    }

    pub fn update_channel(&self) -> Channel {
        self.db
            .meta("update_channel")
            .ok()
            .flatten()
            .and_then(|c| Channel::parse(&c))
            .unwrap_or_default()
    }

    pub fn update_auto(&self) -> bool {
        self.db.meta("update_auto").ok().flatten().as_deref() != Some("0")
    }

    pub fn set_update_options(&self, channel: Option<Channel>, auto: Option<bool>) -> Result<()> {
        if let Some(c) = channel {
            self.db.set_meta("update_channel", c.name())?;
            // What was found for the other channel no longer counts.
            *self.checked.lock().unwrap_or_else(|e| e.into_inner()) = Checked::default();
        }
        if let Some(a) = auto {
            self.db.set_meta("update_auto", if a { "1" } else { "0" })?;
        }
        Ok(())
    }

    /// Asks the repository for its releases and keeps the newest one of the channel.
    pub async fn check_update(&self) -> Result<()> {
        let Some(settings) = self.update_settings.get().cloned() else {
            bail!("not_configured");
        };
        let channel = self.update_channel();
        let found = tokio::task::spawn_blocking(move || update::fetch_releases(&settings.api))
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let mut checked = self.checked.lock().unwrap_or_else(|e| e.into_inner());
        checked.at = Some(now());
        match found {
            Ok(list) => {
                checked.release = update::pick(list, channel);
                checked.error = None;
                Ok(())
            }
            Err(e) => {
                checked.error = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// What the browser shows about updating.
    pub fn update_status(&self) -> Value {
        let current = crate::version();
        let checked = self.checked.lock().unwrap_or_else(|e| e.into_inner());
        let (state, major) = match &checked.release {
            Some(r) => match update::compare(current, &r.version) {
                Step::Newer { major } => ("newer", major),
                Step::Same => ("same", false),
                Step::Older => ("older", false),
            },
            None => ("none", false),
        };
        let settings = self.update_settings.get();
        // Servers whose agent is older than this panel (only they can be updated from here).
        let outdated: Vec<Value> = self
            .snapshot()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                !s.local && matches!(update::compare(&s.version, current), Step::Newer { .. })
            })
            .map(
                |s| json!({ "id": s.id, "name": s.name, "version": s.version, "online": s.online }),
            )
            .collect();
        json!({
            "outdated": outdated,
            "current": current,
            "configured": settings.is_some(),
            "channel": self.update_channel().name(),
            "auto": self.update_auto(),
            "state": state,
            "major": major,
            "latest": checked.release,
            "checked_at": checked.at,
            "error": checked.error,
            "last_result": settings.and_then(|s| update::read_outcome(&s.dir)),
            "busy": self.ops.running("update"),
        })
    }
}

/// Starts updating the panel to the release the last check found. The operation's id comes
/// back at once; when it is done the panel is about to restart, and the browser reconnects
/// by itself.
pub fn start(hub: &Arc<Hub>, confirm_major: bool) -> Result<String> {
    let Some(settings) = hub.update_settings.get().cloned() else {
        bail!("not_configured");
    };
    let release = hub
        .checked
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .release
        .clone()
        .ok_or_else(|| anyhow!("no_update"))?;
    match update::compare(crate::version(), &release.version) {
        Step::Newer { major } if major && !confirm_major => bail!("major_needs_confirm"),
        Step::Newer { .. } => {}
        _ => bail!("no_update"),
    }
    if !cfg!(target_os = "linux") {
        bail!("no_systemd");
    }
    let id = hub.ops.begin("update", "panel")?;
    let (hub, op) = (hub.clone(), id.clone());
    tokio::spawn(async move {
        let error = run(&hub, &op, &settings, release)
            .await
            .err()
            .map(|e| format!("{e:#}"));
        hub.ops.finish(&op, error, None);
    });
    Ok(id)
}

async fn run(hub: &Arc<Hub>, op: &str, s: &UpdateSettings, release: Release) -> Result<()> {
    let ops = &hub.ops;
    let open = Arc::new(AtomicBool::new(false));
    let (h, o, flag) = (hub.clone(), op.to_owned(), open.clone());
    let (dir, key, rel) = (s.dir.clone(), s.key.clone(), release.clone());
    let staged = tokio::task::spawn_blocking(move || {
        let step = |name: &str| {
            if flag.swap(true, Ordering::SeqCst) {
                h.ops.end(&o, true, None);
            }
            h.ops.run(&o, name);
        };
        update::download(&rel, key.as_deref(), &dir, &step)
    })
    .await
    .map_err(|e| anyhow!("{e}"))?;
    let stage = match staged {
        Ok(dir) => dir,
        Err(e) => {
            ops.end(op, false, Some(format!("{e:#}")));
            return Err(e);
        }
    };
    ops.end(op, true, None);
    ops.run(op, "upd_handoff");
    match handoff(s, &stage, &release.version) {
        Ok(()) => {
            ops.end(op, true, None);
            Ok(())
        }
        Err(e) => {
            ops.end(op, false, Some(format!("{e:#}")));
            Err(e)
        }
    }
}

/// Starts the helper as a transient systemd service of its own, so that it lives on when
/// the panel is stopped to be replaced: the new program swaps the files, restarts the
/// panel, waits for it to answer, and puts the old ones back if it does not.
fn handoff(s: &UpdateSettings, stage: &std::path::Path, version: &str) -> Result<()> {
    let helper = stage.join("kariz-panel");
    let status = std::process::Command::new("systemd-run")
        .args(["--unit", "kariz-panel-update", "--collect", "--quiet", "--"])
        .arg(&helper)
        .arg("update-apply")
        .arg("--stage")
        .arg(stage)
        .args(["--version", version])
        .arg("--config")
        .arg(&s.config)
        .arg("--panel-bin")
        .arg(&s.panel_bin)
        .arg("--kariz-bin")
        .arg(&s.kariz_bin)
        .status()
        .map_err(|_| anyhow!("no_systemd"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("no_systemd")
    }
}

// ---- the other servers ----

use std::time::Duration;

use crate::agent_update::CHUNK;
use crate::hub::LOCAL;
use crate::wire::{Ack, Request, TunnelInfo, UpdateFile};

type Files = Vec<(String, Vec<u8>)>;

/// This panel's release (the archive, its checksum and its signature): the ones kept by the
/// update that brought the panel to this version, or, if they are gone, downloaded again
/// (and checked as always).
async fn release_files(s: &UpdateSettings) -> Result<Files> {
    let version = crate::version();
    let arch = update::arch_name().ok_or_else(|| anyhow!("no_build_for_this_cpu"))?;
    let names = update::asset_names(&format!("v{version}"), arch);
    let read = |dir: &std::path::Path| -> Option<Files> {
        names
            .iter()
            .map(|n| Some((n.clone(), std::fs::read(dir.join(n)).ok()?)))
            .collect()
    };
    if let Some(files) = read(&s.dir.join(version).join("release")) {
        return Ok(files);
    }
    let api = s.api.clone();
    let release = tokio::task::spawn_blocking(move || update::fetch_releases(&api))
        .await
        .map_err(|e| anyhow!("{e}"))??
        .into_iter()
        .find(|r| r.version == version)
        .ok_or_else(|| anyhow!("no_release_for_this_version"))?;
    let (dir, key) = (s.dir.clone(), s.key.clone());
    let staged = tokio::task::spawn_blocking(move || {
        update::download(&release, key.as_deref(), &dir, &|_| {})
    })
    .await
    .map_err(|e| anyhow!("{e}"))??;
    read(&staged.join("release")).ok_or_else(|| anyhow!("no_release_for_this_version"))
}

async fn ack_of(hub: &Hub, server: &str, request: &Request) -> Result<()> {
    let ack: Ack = hub.ask_as(server, request).await?;
    if ack.ok {
        Ok(())
    } else {
        Err(anyhow!(ack.error.unwrap_or_else(|| "failed".into())))
    }
}

/// Sends the release to one agent and tells it to apply it.
async fn send_release(hub: &Arc<Hub>, server: &str, files: &Files) -> Result<()> {
    let version = crate::version().to_owned();
    let listing = files
        .iter()
        .map(|(n, d)| UpdateFile {
            name: n.clone(),
            size: d.len() as u64,
        })
        .collect();
    ack_of(
        hub,
        server,
        &Request::UpdateBegin {
            version: version.clone(),
            files: listing,
        },
    )
    .await?;
    // Several pieces at once: each is one request, and a slow link is not waited on one by one.
    let gate = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set = tokio::task::JoinSet::new();
    for (name, data) in files {
        for (i, piece) in data.chunks(CHUNK as usize).enumerate() {
            let permit = gate.clone().acquire_owned().await?;
            let request = Request::UpdateChunk {
                version: version.clone(),
                name: name.clone(),
                offset: i as u64 * CHUNK,
                data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, piece),
            };
            let (hub, server) = (hub.clone(), server.to_owned());
            set.spawn(async move {
                let _permit = permit;
                ack_of(&hub, &server, &request).await
            });
        }
    }
    while let Some(done) = set.join_next().await {
        done.map_err(|e| anyhow!("{e}"))??;
    }
    ack_of(hub, server, &Request::UpdateApply { version }).await
}

/// Waits for `server` to be connected again at this panel's version.
async fn wait_back(hub: &Hub, server: &str) -> Result<()> {
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if hub
            .snapshot()?
            .iter()
            .any(|s| s.id == server && s.online && s.version == crate::version())
        {
            return Ok(());
        }
    }
    bail!("not_back")
}

/// Restarts a server's tunnels one at a time, waiting for each to be connected again before
/// the next, so a pair never loses both its sides at once.
async fn restart_tunnels(hub: &Arc<Hub>, op: &str, server: &str, label: &str) -> Result<()> {
    let tunnels: Vec<TunnelInfo> = hub.ask_as(server, &Request::Tunnels).await?;
    for t in tunnels.into_iter().filter(|t| t.active == Some(true)) {
        hub.ops.run(op, &format!("upd_tunnel:{label}/{}", t.name));
        let request = Request::TunnelCtl {
            name: t.name.clone(),
            action: "restart".into(),
        };
        if let Err(e) = ack_of(hub, server, &request).await {
            hub.ops.end(op, false, Some(format!("{e:#}")));
            return Err(e);
        }
        let mut connected = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now: Vec<TunnelInfo> = hub
                .ask_as(server, &Request::Tunnels)
                .await
                .unwrap_or_default();
            if now
                .iter()
                .find(|x| x.name == t.name)
                .and_then(|x| x.status.as_ref())
                .is_some_and(|st| st.peer.connected)
            {
                connected = true;
                break;
            }
        }
        if !connected {
            hub.ops.end(op, false, Some("tunnel_not_back".into()));
            bail!("tunnel_not_back:{}", t.name);
        }
        hub.ops.end(op, true, None);
    }
    Ok(())
}

/// Updates the servers whose agent is older than this panel, one at a time (and with
/// `restart` their tunnels, one at a time), and the tunnels of the panel's own server. A
/// server that fails stops the rest, which are left as they were.
pub fn start_servers(hub: &Arc<Hub>, restart: bool) -> Result<String> {
    let Some(settings) = hub.update_settings.get().cloned() else {
        bail!("not_configured");
    };
    if !cfg!(target_os = "linux") && hub.snapshot()?.iter().any(|s| !s.local) {
        bail!("no_systemd");
    }
    let id = hub.ops.begin("update_servers", "servers")?;
    let (hub, op) = (hub.clone(), id.clone());
    tokio::spawn(async move {
        let error = run_servers(&hub, &op, &settings, restart)
            .await
            .err()
            .map(|e| format!("{e:#}"));
        hub.ops.finish(&op, error, None);
    });
    Ok(id)
}

async fn run_servers(hub: &Arc<Hub>, op: &str, s: &UpdateSettings, restart: bool) -> Result<()> {
    let ops = &hub.ops;
    let current = crate::version();
    let behind: Vec<_> = hub
        .snapshot()?
        .into_iter()
        .filter(|x| !x.local && matches!(update::compare(&x.version, current), Step::Newer { .. }))
        .collect();
    if let Some(off) = behind.iter().find(|x| !x.online) {
        bail!("offline:{}", off.id);
    }
    let files = if behind.is_empty() {
        Vec::new()
    } else {
        ops.run(op, "upd_release");
        match release_files(s).await {
            Ok(f) => {
                ops.end(op, true, None);
                f
            }
            Err(e) => {
                ops.end(op, false, Some(format!("{e:#}")));
                return Err(e);
            }
        }
    };
    for server in &behind {
        let fail = |e: anyhow::Error| {
            ops.end(op, false, Some(format!("{e:#}")));
            anyhow!("{}: {e:#}", server.name)
        };
        ops.run(op, &format!("upd_send:{}", server.name));
        if let Err(e) = send_release(hub, &server.id, &files).await {
            return Err(fail(e));
        }
        ops.end(op, true, None);
        ops.run(op, &format!("upd_back:{}", server.name));
        if let Err(e) = wait_back(hub, &server.id).await {
            return Err(fail(e));
        }
        ops.end(op, true, None);
        if restart {
            restart_tunnels(hub, op, &server.id, &server.name)
                .await
                .map_err(|e| anyhow!("{}: {e:#}", server.name))?;
        }
    }
    if restart {
        let local = hub
            .snapshot()?
            .into_iter()
            .find(|x| x.local)
            .map(|x| x.name)
            .unwrap_or_else(|| "this server".to_owned());
        restart_tunnels(hub, op, LOCAL, &local).await?;
    }
    Ok(())
}
