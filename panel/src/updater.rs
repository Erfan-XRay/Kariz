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
        json!({
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
