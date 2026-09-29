//! The agent: what runs on every other server (`kariz-panel agent`). It dials the panel,
//! proves who it is, and answers the panel's requests: a fixed list (docs/PHASE11.md,
//! section 4), none of which runs anything the panel names.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use kariz::mux::{MuxSession, Side};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::collect::{self, Sampler};
use crate::join::{self, JoinCode};
use crate::wire::{Ack, HelloReply, Request, MAX_REQUEST};

pub const DEFAULT_AGENT_CONFIG: &str = "/etc/kariz-panel/agent.toml";
pub const DEFAULT_KARIZ_DIR: &str = "/etc/kariz";

/// The agent's identity and where the panel is: `/etc/kariz-panel/agent.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// The panel's address for agents: `host:port`.
    pub panel: String,
    /// The panel's link token.
    pub link_token: String,
    /// Until the agent is registered: the join secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    /// After that: who it is, and the key it proves it with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Where the Kariz tunnel configs are.
    #[serde(default = "default_kariz_dir")]
    pub kariz_dir: PathBuf,
}

fn default_kariz_dir() -> PathBuf {
    PathBuf::from(DEFAULT_KARIZ_DIR)
}

impl AgentConfig {
    /// The settings a join code stands for.
    pub fn from_join(code: &JoinCode) -> Self {
        Self {
            panel: code.p.clone(),
            link_token: code.t.clone(),
            join: Some(code.j.clone()),
            id: None,
            key: None,
            kariz_dir: default_kariz_dir(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
    }

    /// Writes the file so that only its owner can read it (it holds a key).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    // Written beside the file and renamed, so a crash never leaves half a key.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What the agent proves its key with: the keyed hash of the panel's challenge.
pub fn proof(key_hex: &str, challenge: &str) -> Option<String> {
    let key = hex_to_key(key_hex)?;
    Some(
        blake3::keyed_hash(&key, challenge.as_bytes())
            .to_hex()
            .to_string(),
    )
}

pub fn hex_to_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

/// The agent's state while it runs.
pub struct Agent {
    path: PathBuf,
    config: Mutex<AgentConfig>,
    sampler: Mutex<Sampler>,
}

impl Agent {
    pub fn new(path: &Path, config: AgentConfig) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_path_buf(),
            config: Mutex::new(config),
            sampler: Mutex::new(Sampler::default()),
        })
    }

    fn config(&self) -> AgentConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The answer to one request, as JSON.
    pub async fn handle(&self, request: Request) -> Vec<u8> {
        match request {
            Request::Hello { challenge } => {
                let c = self.config();
                let proof = c.key.as_deref().and_then(|k| proof(k, &challenge));
                to_json(&HelloReply {
                    id: c.id.clone(),
                    proof,
                    join: c.join.clone(),
                    hostname: collect::hostname(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    arch: std::env::consts::ARCH.to_owned(),
                })
            }
            Request::Enroll { id, key } => {
                let saved = {
                    let mut c = self.config.lock().unwrap_or_else(|e| e.into_inner());
                    // Only an agent still waiting to be registered takes an identity.
                    if c.join.is_none() || hex_to_key(&key).is_none() {
                        Err(anyhow::anyhow!("not waiting to be registered"))
                    } else {
                        let mut next = c.clone();
                        next.join = None;
                        next.id = Some(id);
                        next.key = Some(key);
                        next.save(&self.path).map(|()| *c = next)
                    }
                };
                match saved {
                    Ok(()) => to_json(&Ack {
                        ok: true,
                        error: None,
                    }),
                    Err(e) => to_json(&Ack {
                        ok: false,
                        error: Some(format!("{e:#}")),
                    }),
                }
            }
            Request::Health => {
                let health = self
                    .sampler
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .sample();
                to_json(&health)
            }
            Request::Tunnels => to_json(&collect::tunnels(&self.config().kariz_dir).await),
        }
    }

    /// Answers the requests on one session until it closes.
    pub async fn serve(self: &Arc<Self>, session: MuxSession) {
        let session = Arc::new(session);
        while let Some((stream, syn)) = session.accept().await {
            let agent = self.clone();
            tokio::spawn(async move {
                let reply = match serde_json::from_slice::<Request>(&syn) {
                    Ok(request) if syn.len() <= MAX_REQUEST => agent.handle(request).await,
                    _ => serde_json::to_vec(&Ack {
                        ok: false,
                        error: Some("unknown request".into()),
                    })
                    .unwrap_or_default(),
                };
                if stream.send(Bytes::from(reply)).await.is_ok() {
                    let _ = stream.finish();
                }
            });
        }
    }

    /// Keeps a session to the panel up, for ever.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let c = self.config();
        if c.join.is_none() && (c.id.is_none() || c.key.is_none()) {
            bail!(
                "the agent has no join code and no identity: run `kariz-panel agent --join CODE`"
            );
        }
        let dialer = kariz::link::Dialer::new(&c.panel, &c.link_token)?;
        let mut backoff = Duration::from_secs(1);
        loop {
            match dialer.connect(Side::Server).await {
                Ok(session) => {
                    info!(panel = %c.panel, "connected to the panel");
                    backoff = Duration::from_secs(1);
                    let started = std::time::Instant::now();
                    self.serve(session).await;
                    warn!("the link to the panel ended; reconnecting");
                    if started.elapsed() < Duration::from_secs(2) {
                        tokio::time::sleep(backoff).await;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "could not connect to the panel");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// Reads a join code, writes the agent's settings from it, and returns them.
pub fn enroll_from_code(code_text: &str, path: &Path) -> Result<AgentConfig> {
    let code = join::decode(code_text)?;
    let config = AgentConfig::from_join(&code);
    config.save(path)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Health;

    fn key() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn a_proof_needs_the_key_and_the_challenge() {
        let a = proof(&key(), "challenge one").unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(proof(&key(), "challenge one").unwrap(), a);
        assert_ne!(proof(&key(), "challenge two").unwrap(), a);
        assert_ne!(proof(&"cd".repeat(32), "challenge one").unwrap(), a);
        assert_eq!(proof("short", "x"), None);
        assert_eq!(proof(&"zz".repeat(32), "x"), None);
    }

    #[tokio::test]
    async fn an_agent_is_enrolled_once_and_keeps_its_identity_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.toml");
        let code = JoinCode {
            p: "127.0.0.1:1".into(),
            t: "t".repeat(64),
            j: "j".repeat(32),
            n: None,
        };
        let config = enroll_from_code(&join::encode(&code), &path).unwrap();
        assert_eq!(config.join.as_deref(), Some(code.j.as_str()));
        let agent = Agent::new(&path, config);

        // Before enrolling, it shows the join secret and no proof.
        let hello = |agent: &Arc<Agent>| {
            let agent = agent.clone();
            async move {
                let raw = agent
                    .handle(Request::Hello {
                        challenge: "c".into(),
                    })
                    .await;
                serde_json::from_slice::<HelloReply>(&raw).unwrap()
            }
        };
        let before = hello(&agent).await;
        assert_eq!((before.id, before.proof), (None, None));
        assert_eq!(before.join.as_deref(), Some(code.j.as_str()));

        let ack: Ack = serde_json::from_slice(
            &agent
                .handle(Request::Enroll {
                    id: "id1".into(),
                    key: key(),
                })
                .await,
        )
        .unwrap();
        assert!(ack.ok, "{ack:?}");
        let after = hello(&agent).await;
        assert_eq!(after.id.as_deref(), Some("id1"));
        assert_eq!(after.proof, proof(&key(), "c"));
        assert_eq!(after.join, None);

        // The identity is on disk, and a second enrollment (a hijack) is refused.
        let saved = AgentConfig::load(&path).unwrap();
        assert_eq!((saved.id.as_deref(), saved.join), (Some("id1"), None));
        let again: Ack = serde_json::from_slice(
            &agent
                .handle(Request::Enroll {
                    id: "evil".into(),
                    key: "cd".repeat(32),
                })
                .await,
        )
        .unwrap();
        assert!(!again.ok);
        assert_eq!(AgentConfig::load(&path).unwrap().id.as_deref(), Some("id1"));
        // A key that is not 32 bytes of hex is refused too.
        let fresh = Agent::new(&path, AgentConfig::from_join(&code));
        let bad: Ack = serde_json::from_slice(
            &fresh
                .handle(Request::Enroll {
                    id: "x".into(),
                    key: "short".into(),
                })
                .await,
        )
        .unwrap();
        assert!(!bad.ok);
    }

    #[tokio::test]
    async fn health_and_tunnels_answer_as_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AgentConfig::from_join(&JoinCode {
            p: "x:1".into(),
            t: "t".repeat(64),
            j: "j".into(),
            n: None,
        });
        config.kariz_dir = dir.path().to_path_buf();
        let agent = Agent::new(&dir.path().join("agent.toml"), config);
        let health: Health = serde_json::from_slice(&agent.handle(Request::Health).await).unwrap();
        let _ = health; // all None off Linux
        let tunnels: Vec<crate::wire::TunnelInfo> =
            serde_json::from_slice(&agent.handle(Request::Tunnels).await).unwrap();
        assert!(tunnels.is_empty());
    }
}
