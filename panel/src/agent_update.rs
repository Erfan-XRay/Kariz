//! An agent updating itself when the panel sends it a release.
//! The panel downloaded the release once; the agent never needs the internet. It is sent in
//! pieces (a request holds 16 KB), checked again here with the release key, unpacked, and
//! handed to a helper that swaps the programs, restarts the agent's service and waits for the
//! new agent to reach the panel again, putting the old programs back if it does not.

use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;

use crate::sign;
use crate::update::{self, Host, Step};
use crate::wire::UpdateFile;

/// Bytes of the release in one request (base64 makes it a third larger).
pub const CHUNK: u64 = 9000;
const MAX_ARCHIVE: u64 = 200 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;

/// Starts the helper. The real one is `systemd-run`; the tests record what it would run.
pub trait Launcher: Send + Sync {
    fn launch(&self, program: &Path, args: Vec<String>) -> Result<()>;
}

/// A transient systemd service of its own, so the helper lives on when the agent is stopped.
pub struct SystemdRun;

impl Launcher for SystemdRun {
    fn launch(&self, program: &Path, args: Vec<String>) -> Result<()> {
        let status = std::process::Command::new("systemd-run")
            .args(["--unit", "kariz-agent-update", "--collect", "--quiet", "--"])
            .arg(program)
            .args(args)
            .status()
            .map_err(|_| anyhow!("no_systemd"))?;
        if status.success() {
            Ok(())
        } else {
            bail!("no_systemd")
        }
    }
}

struct Incoming {
    /// Name to declared size, and which pieces have arrived.
    files: HashMap<String, (u64, HashSet<u64>)>,
}

pub struct AgentUpdate {
    /// Where releases are received and unpacked.
    dir: PathBuf,
    /// The agent's settings file, which the helper reads.
    config: PathBuf,
    /// A release key of the agent's own, instead of the built-in one.
    key: Option<String>,
    panel_bin: PathBuf,
    kariz_bin: PathBuf,
    launcher: Box<dyn Launcher>,
    incoming: Mutex<HashMap<String, Incoming>>,
}

impl AgentUpdate {
    pub fn new(
        dir: PathBuf,
        config: PathBuf,
        key: Option<String>,
        panel_bin: PathBuf,
        kariz_bin: PathBuf,
        launcher: Box<dyn Launcher>,
    ) -> Self {
        Self {
            dir,
            config,
            key,
            panel_bin,
            kariz_bin,
            launcher,
            incoming: Mutex::new(HashMap::new()),
        }
    }

    fn version_dir(&self, version: &str) -> Result<PathBuf> {
        // The version becomes a folder name: only what a version looks like is allowed.
        if update::Version::parse(version).is_none() || version.contains(['/', '\\']) {
            bail!("bad_version");
        }
        Ok(self.dir.join(version))
    }

    /// A release is about to arrive: three files, with these sizes.
    pub fn begin(&self, version: &str, files: &[UpdateFile]) -> Result<()> {
        self.begin_inner(version, files)
            .inspect_err(|e| tracing::warn!(version, error = %e, "a release was refused"))
    }

    fn begin_inner(&self, version: &str, files: &[UpdateFile]) -> Result<()> {
        let dir = self.version_dir(version)?;
        if !matches!(
            update::compare(crate::version(), version),
            Step::Newer { .. }
        ) {
            bail!("already_current");
        }
        let arch = update::arch_name().ok_or_else(|| anyhow!("no_build_for_this_cpu"))?;
        let names = update::asset_names(&format!("v{version}"), arch);
        let mut sizes = HashMap::new();
        for f in files {
            let limit = if f.name == names[0] {
                MAX_ARCHIVE
            } else {
                MAX_SMALL
            };
            if !names.contains(&f.name)
                || f.size == 0
                || f.size > limit
                || sizes.insert(f.name.clone(), f.size).is_some()
            {
                bail!("bad_files");
            }
        }
        if sizes.len() != 3 {
            bail!("bad_files");
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let release = dir.join("release");
        std::fs::create_dir_all(&release)?;
        for (name, size) in &sizes {
            let f = std::fs::File::create(release.join(name))?;
            f.set_len(*size)?;
        }
        self.incoming
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                version.to_owned(),
                Incoming {
                    files: sizes
                        .into_iter()
                        .map(|(n, s)| (n, (s, HashSet::new())))
                        .collect(),
                },
            );
        Ok(())
    }

    /// One piece: `CHUNK` bytes at a multiple of `CHUNK` (the last one shorter).
    pub fn chunk(&self, version: &str, name: &str, offset: u64, data_b64: &str) -> Result<()> {
        let dir = self.version_dir(version)?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|_| anyhow!("bad_chunk"))?;
        let mut incoming = self.incoming.lock().unwrap_or_else(|e| e.into_inner());
        let entry = incoming
            .get_mut(version)
            .ok_or_else(|| anyhow!("not_begun"))?;
        let (size, seen) = entry
            .files
            .get_mut(name)
            .ok_or_else(|| anyhow!("bad_files"))?;
        let expected = CHUNK.min(size.saturating_sub(offset));
        if offset % CHUNK != 0 || offset >= *size || data.len() as u64 != expected {
            bail!("bad_chunk");
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join("release").join(name))
            .context("not_begun")?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&data)?;
        seen.insert(offset / CHUNK);
        Ok(())
    }

    fn complete(&self, version: &str) -> bool {
        self.incoming
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(version)
            .is_some_and(|i| {
                i.files
                    .values()
                    .all(|(size, seen)| seen.len() as u64 == size.div_ceil(CHUNK))
            })
    }

    /// Checks what arrived, unpacks it, and hands over to the helper.
    pub fn apply(&self, version: &str) -> Result<()> {
        self.apply_inner(version)
            .inspect(|()| tracing::info!(version, "a release was checked and handed to the helper"))
            .inspect_err(|e| tracing::warn!(version, error = %e, "a release could not be applied"))
    }

    fn apply_inner(&self, version: &str) -> Result<()> {
        let dir = self.version_dir(version)?;
        if !self.complete(version) {
            bail!("incomplete");
        }
        let arch = update::arch_name().ok_or_else(|| anyhow!("no_build_for_this_cpu"))?;
        let names = update::asset_names(&format!("v{version}"), arch);
        let release = dir.join("release");
        let read = |n: &String| std::fs::read(release.join(n)).context("incomplete");
        let key = sign::release_key(self.key.as_deref())?;
        sign::verify_archive(
            &key,
            &names[0],
            &read(&names[0])?,
            &read(&names[1])?,
            &read(&names[2])?,
        )
        .inspect_err(|_| {
            // What did not verify is not kept.
            let _ = std::fs::remove_dir_all(&dir);
        })?;
        update::extract(&read(&names[0])?, &dir)?;
        self.incoming
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(version);
        let stamp = self.config.with_file_name("connected");
        let args = vec![
            "agent-update-apply".to_owned(),
            "--stage".to_owned(),
            dir.display().to_string(),
            "--version".to_owned(),
            version.to_owned(),
            "--stamp".to_owned(),
            stamp.display().to_string(),
            "--panel-bin".to_owned(),
            self.panel_bin.display().to_string(),
            "--kariz-bin".to_owned(),
            self.kariz_bin.display().to_string(),
            // The agent still has to answer the panel that the hand-over went well.
            "--delay".to_owned(),
            "3".to_owned(),
        ];
        self.launcher.launch(&dir.join("kariz-panel"), args)
    }
}

/// The agent's side of "did the new program work": its service is running and it has reached
/// the panel again since the swap (the agent touches `stamp` each time it connects).
pub struct AgentHost {
    pub unit: String,
    pub stamp: PathBuf,
    pub since: SystemTime,
}

impl Host for AgentHost {
    fn restart(&self) -> Result<()> {
        let status = std::process::Command::new("systemctl")
            .args(["restart", &self.unit])
            .status()
            .context("failed to run systemctl")?;
        if status.success() {
            Ok(())
        } else {
            bail!("systemctl restart {} failed", self.unit)
        }
    }

    fn answers(&self, _version: &str) -> bool {
        let fresh = std::fs::metadata(&self.stamp)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= self.since);
        fresh
            && std::process::Command::new("systemctl")
                .args(["is-active", "--quiet", &self.unit])
                .status()
                .is_ok_and(|s| s.success())
    }
}

/// Touches the file the helper watches: the agent is connected to the panel.
pub fn touch(stamp: &Path) {
    let _ = std::fs::write(stamp, crate::auth::now().to_string());
}

/// How long the helper gives the new agent to reach the panel.
pub const WAIT: Duration = Duration::from_secs(40);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// What the helper would have been started with: its program and arguments.
    type Launches = Arc<Mutex<Vec<(PathBuf, Vec<String>)>>>;

    #[derive(Default)]
    struct Recorder(Launches);

    impl Launcher for Recorder {
        fn launch(&self, program: &Path, args: Vec<String>) -> Result<()> {
            self.0.lock().unwrap().push((program.to_path_buf(), args));
            Ok(())
        }
    }

    fn archive(version: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in [
            ("kariz-x/kariz", &b"core"[..]),
            ("kariz-x/kariz-panel", &b"panel"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, data).unwrap();
        }
        let _ = version;
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    struct Release {
        files: Vec<(String, Vec<u8>)>,
        key: String,
    }

    fn release(version: &str) -> Release {
        let arch = update::arch_name().unwrap();
        let names = update::asset_names(&format!("v{version}"), arch);
        let (private, public) = sign::generate().unwrap();
        let archive = archive(version);
        let sha = format!("{}  {}\n", sign::sha256_hex(&archive), names[0]).into_bytes();
        let sig = sign::sign(&private, &sha).unwrap();
        Release {
            files: vec![
                (names[0].clone(), archive),
                (names[1].clone(), sha),
                (names[2].clone(), sig),
            ],
            key: sign::hex(&public),
        }
    }

    fn updater(key: Option<String>) -> (tempfile::TempDir, AgentUpdate, Launches) {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::default();
        let log = rec.0.clone();
        let u = AgentUpdate::new(
            dir.path().join("updates"),
            dir.path().join("agent.toml"),
            key,
            PathBuf::from("/usr/local/bin/kariz-panel"),
            PathBuf::from("/usr/local/bin/kariz"),
            Box::new(rec),
        );
        (dir, u, log)
    }

    fn send(u: &AgentUpdate, version: &str, r: &Release) -> Result<()> {
        let files: Vec<UpdateFile> = r
            .files
            .iter()
            .map(|(n, d)| UpdateFile {
                name: n.clone(),
                size: d.len() as u64,
            })
            .collect();
        u.begin(version, &files)?;
        for (name, data) in &r.files {
            for (i, piece) in data.chunks(CHUNK as usize).enumerate() {
                u.chunk(
                    version,
                    name,
                    i as u64 * CHUNK,
                    &base64::engine::general_purpose::STANDARD.encode(piece),
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn a_release_that_arrives_whole_and_signed_is_unpacked_and_handed_to_the_helper() {
        let r = release("99.0.0");
        let (dir, u, log) = updater(Some(r.key.clone()));
        send(&u, "99.0.0", &r).unwrap();
        u.apply("99.0.0").unwrap();
        let stage = dir.path().join("updates/99.0.0");
        assert_eq!(std::fs::read(stage.join("kariz-panel")).unwrap(), b"panel");
        assert_eq!(std::fs::read(stage.join("kariz")).unwrap(), b"core");
        let launched = log.lock().unwrap();
        assert_eq!(launched.len(), 1);
        assert_eq!(
            launched[0].0,
            stage.join("kariz-panel"),
            "the new program is the helper"
        );
        assert_eq!(launched[0].1[0], "agent-update-apply");
        assert!(launched[0].1.contains(&"99.0.0".to_owned()));
    }

    #[test]
    fn a_release_signed_by_another_key_is_refused_deleted_and_never_launched() {
        let r = release("99.0.0");
        let other = release("99.0.0");
        let (dir, u, log) = updater(Some(other.key));
        send(&u, "99.0.0", &r).unwrap();
        let err = u.apply("99.0.0").unwrap_err();
        assert_eq!(format!("{err:#}"), "bad_signature");
        assert!(
            !dir.path().join("updates/99.0.0").exists(),
            "what did not verify is not kept"
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn a_damaged_piece_or_a_missing_one_stops_it() {
        let r = release("99.0.0");
        let (_dir, u, log) = updater(Some(r.key.clone()));
        let files: Vec<UpdateFile> = r
            .files
            .iter()
            .map(|(n, d)| UpdateFile {
                name: n.clone(),
                size: d.len() as u64,
            })
            .collect();
        u.begin("99.0.0", &files).unwrap();
        // apply before anything arrived
        assert_eq!(
            format!("{:#}", u.apply("99.0.0").unwrap_err()),
            "incomplete"
        );
        let (name, data) = &r.files[1];
        let b64 = |d: &[u8]| base64::engine::general_purpose::STANDARD.encode(d);
        // a piece of the wrong length, at a wrong offset, past the end, for an unknown file
        assert!(u.chunk("99.0.0", name, 0, &b64(&data[..3])).is_err());
        assert!(u.chunk("99.0.0", name, 1, &b64(data)).is_err());
        assert!(u.chunk("99.0.0", name, 9000, &b64(data)).is_err());
        assert!(u.chunk("99.0.0", "other", 0, &b64(data)).is_err());
        assert!(u.chunk("99.0.0", name, 0, "not base64!!").is_err());
        assert!(
            u.chunk("98.0.0", name, 0, &b64(data)).is_err(),
            "a version that never began"
        );
        // one file sent, the others missing
        u.chunk("99.0.0", name, 0, &b64(data)).unwrap();
        assert_eq!(
            format!("{:#}", u.apply("99.0.0").unwrap_err()),
            "incomplete"
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn what_begin_accepts_is_only_this_releases_three_files() {
        let r = release("99.0.0");
        let (_dir, u, _) = updater(Some(r.key.clone()));
        let f = |n: &str, s: u64| UpdateFile {
            name: n.into(),
            size: s,
        };
        let names: Vec<String> = r.files.iter().map(|x| x.0.clone()).collect();
        assert!(
            u.begin("99.0.0", &[f(&names[0], 10), f(&names[1], 10)])
                .is_err(),
            "one is missing"
        );
        assert!(u
            .begin(
                "99.0.0",
                &[f(&names[0], 10), f(&names[1], 10), f("../evil", 10)]
            )
            .is_err());
        assert!(u
            .begin(
                "99.0.0",
                &[f(&names[0], 10), f(&names[1], 10), f(&names[2], 0)]
            )
            .is_err());
        assert!(u
            .begin(
                "99.0.0",
                &[f(&names[0], 10), f(&names[1], 10), f(&names[0], 10)]
            )
            .is_err());
        assert!(u
            .begin(
                "99.0.0",
                &[
                    f(&names[0], 300 * 1024 * 1024),
                    f(&names[1], 10),
                    f(&names[2], 10)
                ]
            )
            .is_err());
        assert!(u
            .begin(
                "../../etc",
                &[f(&names[0], 10), f(&names[1], 10), f(&names[2], 10)]
            )
            .is_err());
        assert!(u
            .begin(
                "99.0.0",
                &[f(&names[0], 10), f(&names[1], 10), f(&names[2], 10)]
            )
            .is_ok());
    }

    #[test]
    fn an_agent_does_not_take_the_version_it_already_has_or_an_older_one() {
        let (_dir, u, _) = updater(None);
        let f = vec![UpdateFile {
            name: "x".into(),
            size: 1,
        }];
        for v in [crate::version(), "0.0.1"] {
            assert_eq!(
                format!("{:#}", u.begin(v, &f).unwrap_err()),
                "already_current",
                "{v}"
            );
        }
    }
}
