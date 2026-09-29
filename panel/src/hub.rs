//! The panel's side of the agents: it accepts their links, checks who they are, asks them
//! for their health and tunnels every couple of seconds, and keeps the latest of it for
//! the API (docs/PHASE11.md, section 4).
//!
//! An agent proves itself in one of two ways. A registered one answers the panel's
//! random challenge with a keyed hash only its key makes. A new one shows the join
//! secret from its join code, which works once; the panel then makes it an identity (an
//! id and a key), sends it over the link, and forgets the secret. The link token in the
//! handshake is the first gate; the key is the second, so a leaked token alone opens no
//! server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use kariz::mux::{MuxSession, Side};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tracing::{debug, info, warn};

use crate::agent::{Agent, AgentConfig};
use crate::auth::{hash_token, now};
use crate::collect::{self, Sampler};
use crate::config::random_hex;
use crate::db::Db;
use crate::join::{self, JoinCode, JOIN_TTL};
use crate::manage::{Services, Systemd};
use crate::wire::{Ack, Health, HelloReply, Request, TunnelInfo, MAX_REPLY};

/// How often the panel asks an agent for its state.
pub const POLL: Duration = Duration::from_secs(2);
/// A request that takes longer than this ends the link.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One tunnel as the panel shows it: its config and state, and its traffic as a rate.
#[derive(Debug, Clone, Serialize)]
pub struct TunnelView {
    #[serde(flatten)]
    pub info: TunnelInfo,
    /// Mbit/s, both directions together, from two readings of the daemon's counters.
    pub rate_mbps: Option<f64>,
}

/// A server as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub local: bool,
    pub online: bool,
    pub version: String,
    pub arch: String,
    pub hostname: String,
    pub seen_secs: Option<u64>,
    pub health: Option<Health>,
    pub tunnels: Vec<TunnelView>,
}

#[derive(Default)]
struct Live {
    online: bool,
    seen: Option<Instant>,
    hostname: String,
    version: String,
    arch: String,
    health: Option<Health>,
    tunnels: Vec<TunnelView>,
    /// Bytes counted at the last reading of each tunnel, for the rates.
    last_bytes: HashMap<String, (u64, Instant)>,
    session: Option<Arc<MuxSession>>,
    /// Whether each tunnel was connected at the last reading, to notice it changing.
    connected: HashMap<String, bool>,
}

pub struct Hub {
    db: Db,
    kariz_dir: PathBuf,
    live: Mutex<HashMap<String, Live>>,
    /// The panel's own server, which answers the same requests as an agent, in-process.
    local: Arc<Agent>,
    services: Arc<dyn Services>,
    /// The running and recent tunnel operations (`crate::pair`).
    pub ops: crate::pair::Ops,
    /// Charts and events.
    pub history: crate::history::History,
    /// How long a new tunnel has to connect (shortened in tests).
    pub connect_wait: Duration,
}

pub const LOCAL: &str = "local";

impl Hub {
    pub fn new(db: Db, kariz_dir: PathBuf) -> Arc<Self> {
        Self::with_services(db, kariz_dir, Arc::new(Systemd))
    }

    /// A hub whose own server runs its tunnels through `services`.
    pub fn with_services(db: Db, kariz_dir: PathBuf, services: Arc<dyn Services>) -> Arc<Self> {
        Self::with_options(db, kariz_dir, services, crate::pair::CONNECT_WAIT)
    }

    /// Also sets how long a new tunnel has to connect.
    pub fn with_options(
        db: Db,
        kariz_dir: PathBuf,
        services: Arc<dyn Services>,
        connect_wait: Duration,
    ) -> Arc<Self> {
        let config = AgentConfig {
            panel: String::new(),
            link_token: String::new(),
            join: None,
            id: None,
            key: None,
            kariz_dir: kariz_dir.clone(),
            services: Default::default(),
        };
        Arc::new(Self {
            history: crate::history::History::new(db.clone()),
            db,
            local: Agent::with_services(Path::new(""), config, services.clone()),
            services,
            ops: crate::pair::Ops::default(),
            connect_wait,
            kariz_dir,
            live: Mutex::new(HashMap::new()),
        })
    }

    /// One request to a server (the panel's own, or an agent that is connected), and its
    /// raw answer.
    pub async fn ask(&self, server: &str, request: &Request) -> Result<Vec<u8>> {
        if server == LOCAL {
            return Ok(self.local.handle(request.clone()).await);
        }
        let session = self
            .live()
            .get(server)
            .filter(|l| l.online)
            .and_then(|l| l.session.clone())
            .ok_or_else(|| anyhow!("that server is not connected"))?;
        request_on(&session, request).await
    }

    /// Like [`Hub::ask`], with the answer read as `T`.
    pub async fn ask_as<T: serde::de::DeserializeOwned>(
        &self,
        server: &str,
        request: &Request,
    ) -> Result<T> {
        let raw = self.ask(server, request).await?;
        serde_json::from_slice(&raw).map_err(|_| match serde_json::from_slice::<Ack>(&raw) {
            Ok(Ack { error: Some(e), .. }) => anyhow!(e),
            _ => anyhow!("the server's answer was not understood"),
        })
    }

    fn live(&self) -> MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The panel's link token, made the first time it is asked for.
    pub fn link_token(&self) -> Result<String> {
        if let Some(t) = self.db.meta("link_token")? {
            return Ok(t);
        }
        let token = crate::auth::new_token()?;
        self.db.set_meta("link_token", &token)?;
        Ok(token)
    }

    /// A join code for a new server: `name` (optional) is what it will be called, and
    /// `panel` where it should dial.
    pub fn create_join(&self, name: Option<&str>, panel: &str) -> Result<String> {
        let secret = random_hex(16)?;
        let t = now();
        self.db.conn().execute(
            "INSERT INTO joins (hash, name, created, expires, used) VALUES (?1, ?2, ?3, ?4, 0)",
            params![hash_token(&secret), name.unwrap_or(""), t, t + JOIN_TTL],
        )?;
        Ok(join::encode(&JoinCode {
            p: panel.to_owned(),
            t: self.link_token()?,
            j: secret,
            n: name.map(str::to_owned),
        }))
    }

    /// Spends a join secret: the name it carried, once, if it is valid.
    fn spend_join(&self, secret: &str) -> Result<Option<String>> {
        let conn = self.db.conn();
        let t = now();
        conn.execute("DELETE FROM joins WHERE expires < ?1 - 86400", [t])?;
        let hash = hash_token(secret);
        let changed = conn.execute(
            "UPDATE joins SET used = 1 WHERE hash = ?1 AND used = 0 AND expires > ?2",
            params![hash, t],
        )?;
        if changed != 1 {
            return Ok(None);
        }
        Ok(Some(conn.query_row(
            "SELECT name FROM joins WHERE hash = ?1",
            [&hash],
            |r| r.get(0),
        )?))
    }

    fn unique_name(&self, wanted: &str) -> Result<String> {
        let base = if join::valid_name(wanted) {
            wanted.to_owned()
        } else {
            format!("server-{}", random_hex(2)?)
        };
        let conn = self.db.conn();
        let exists = |n: &str| -> rusqlite::Result<bool> {
            conn.query_row("SELECT count(*) FROM servers WHERE name = ?1", [n], |r| {
                r.get::<_, i64>(0)
            })
            .map(|c| c > 0)
        };
        let mut name = base.clone();
        let mut n = 1;
        while exists(&name)? {
            n += 1;
            name = format!("{base}-{n}");
        }
        Ok(name)
    }

    /// Registers a new server: an id, a key and a name.
    fn register(&self, name: &str, hello: &HelloReply) -> Result<(String, String, String)> {
        let id = random_hex(8)?;
        let key = random_hex(32)?;
        let name = self.unique_name(name)?;
        self.db.conn().execute(
            "INSERT INTO servers (id, name, key, created, last_seen, version, arch, host) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)",
            params![id, name, key, now(), hello.version, hello.arch, hello.hostname],
        )?;
        Ok((id, key, name))
    }

    fn server_key(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .conn()
            .query_row("SELECT key FROM servers WHERE id = ?1", [id], |r| r.get(0))
            .optional()?)
    }

    /// Removes a server: its row, and its link if it has one. False if there is none.
    pub fn remove(&self, id: &str) -> Result<bool> {
        if id == LOCAL {
            return Ok(false);
        }
        let removed = self
            .db
            .conn()
            .execute("DELETE FROM servers WHERE id = ?1", [id])?
            == 1;
        if let Some(live) = self.live().remove(id) {
            if let Some(session) = live.session {
                session.close();
            }
        }
        Ok(removed)
    }

    // ---- the agents' links ----

    /// Accepts agents for ever. Each connection gets its own task, so a slow or hostile
    /// peer never holds up the others.
    pub async fn serve_agents(self: Arc<Self>, acceptor: kariz::link::Acceptor) {
        loop {
            match acceptor.accept().await {
                Ok(pending) => {
                    let hub = self.clone();
                    let peer = pending.peer();
                    tokio::spawn(async move {
                        match pending.establish(Side::Client).await {
                            Ok(session) => {
                                if let Err(e) = hub.run_session(Arc::new(session)).await {
                                    debug!(%peer, error = %e, "an agent link ended");
                                }
                            }
                            Err(e) => debug!(%peer, error = %e, "an agent did not authenticate"),
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "accepting an agent failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    async fn run_session(&self, session: Arc<MuxSession>) -> Result<()> {
        let (id, hello) = tokio::time::timeout(Duration::from_secs(20), self.identify(&session))
            .await
            .map_err(|_| anyhow!("the agent did not identify itself in time"))??;
        info!(server = %id, "an agent is connected");
        {
            let mut live = self.live();
            let entry = live.entry(id.clone()).or_default();
            // A new link replaces an old one that has not noticed it is dead.
            if let Some(old) = entry.session.replace(session.clone()) {
                old.close();
            }
            entry.online = true;
            entry.hostname = hello.hostname.clone();
            entry.version = hello.version.clone();
            entry.arch = hello.arch.clone();
        }
        self.history.event("server_up", &id, "");
        let result = self.poll(&id, &session).await;
        let mut live = self.live();
        if let Some(entry) = live.get_mut(&id) {
            // Only if this link is still the current one.
            if entry
                .session
                .as_ref()
                .is_some_and(|s| Arc::ptr_eq(s, &session))
            {
                entry.online = false;
                entry.session = None;
                self.history.event("server_down", &id, "");
            }
        }
        result
    }

    /// Checks who is on the other end of a new link: a registered agent proves its key, a
    /// new one is registered with its join secret.
    async fn identify(&self, session: &Arc<MuxSession>) -> Result<(String, HelloReply)> {
        let challenge = random_hex(16)?;
        let raw = request_on(
            session,
            &Request::Hello {
                challenge: challenge.clone(),
            },
        )
        .await?;
        let hello: HelloReply = serde_json::from_slice(&raw)?;

        if let (Some(id), Some(proof)) = (&hello.id, &hello.proof) {
            let key = self
                .server_key(id)?
                .ok_or_else(|| anyhow!("an agent with an unknown id"))?;
            let expected =
                crate::agent::proof(&key, &challenge).ok_or_else(|| anyhow!("a bad stored key"))?;
            // The keyed hash is compared in constant time.
            let ok = blake3::Hash::from_hex(&expected)?
                == blake3::Hash::from_hex(proof).map_err(|_| anyhow!("a malformed proof"))?;
            if !ok {
                bail!("an agent with a wrong proof for {id}");
            }
            self.db.conn().execute(
                "UPDATE servers SET last_seen = ?2, version = ?3, arch = ?4, host = ?5 WHERE id = ?1",
                params![id, now(), hello.version, hello.arch, hello.hostname],
            )?;
            return Ok((id.clone(), hello));
        }
        let Some(secret) = &hello.join else {
            bail!("an agent with neither an identity nor a join secret");
        };
        let Some(wanted) = self.spend_join(secret)? else {
            bail!("an agent with a join secret that is wrong, used or expired");
        };
        let wanted = if wanted.is_empty() {
            hello.hostname.clone()
        } else {
            wanted
        };
        let (id, key, name) = self.register(&wanted, &hello)?;
        let raw = request_on(
            session,
            &Request::Enroll {
                id: id.clone(),
                key,
            },
        )
        .await?;
        let ack: Ack = serde_json::from_slice(&raw)?;
        if !ack.ok {
            let _ = self.remove(&id);
            bail!(
                "the agent did not keep its identity: {}",
                ack.error.unwrap_or_default()
            );
        }
        let _ = self
            .db
            .audit("hub", None, &format!("registered server {name} ({id})"));
        Ok((id, hello))
    }

    /// Asks the agent for its state every [`POLL`] until the link ends.
    async fn poll(&self, id: &str, session: &Arc<MuxSession>) -> Result<()> {
        loop {
            let health: Health =
                serde_json::from_slice(&request_on(session, &Request::Health).await?)?;
            let tunnels: Vec<TunnelInfo> =
                serde_json::from_slice(&request_on(session, &Request::Tunnels).await?)?;
            self.update(id, Some(health), tunnels);
            let _ = self.db.conn().execute(
                "UPDATE servers SET last_seen = ?2 WHERE id = ?1",
                params![id, now()],
            );
            tokio::select! {
                () = tokio::time::sleep(POLL) => {}
                () = session.closed() => bail!("the link closed"),
            }
        }
    }

    fn update(&self, id: &str, health: Option<Health>, tunnels: Vec<TunnelInfo>) {
        let mut live = self.live();
        let entry = live.entry(id.to_owned()).or_default();
        let t = Instant::now();
        let at = now();
        entry.seen = Some(t);
        if let Some(h) = &health {
            let h_ = &self.history;
            if let Some(v) = h.cpu_pct {
                h_.record(&format!("srv:{id}:cpu"), at, v);
            }
            if let (Some(u), Some(total)) = (h.mem_used, h.mem_total) {
                if total > 0 {
                    h_.record(
                        &format!("srv:{id}:mem"),
                        at,
                        u as f64 * 100.0 / total as f64,
                    );
                }
            }
            if let Some(v) = h.rx_bps {
                h_.record(&format!("srv:{id}:rx"), at, v * 8.0 / 1e6);
            }
            if let Some(v) = h.tx_bps {
                h_.record(&format!("srv:{id}:tx"), at, v * 8.0 / 1e6);
            }
        }
        entry.health = health;
        entry.tunnels = tunnels
            .into_iter()
            .map(|info| {
                let bytes = info
                    .status
                    .as_ref()
                    .map(|s| s.totals.bytes_up + s.totals.bytes_down);
                let rate = match (bytes, entry.last_bytes.get(&info.name)) {
                    (Some(now_b), Some(&(before, at))) => {
                        let secs = t.duration_since(at).as_secs_f64();
                        (secs > 0.05)
                            .then(|| now_b.saturating_sub(before) as f64 * 8.0 / secs / 1e6)
                    }
                    _ => None,
                };
                match bytes {
                    Some(b) => entry.last_bytes.insert(info.name.clone(), (b, t)),
                    None => entry.last_bytes.remove(&info.name),
                };
                if let Some(status) = &info.status {
                    let up = status.peer.connected;
                    // Changes worth telling; the first reading of a tunnel is not one.
                    match entry.connected.insert(info.name.clone(), up) {
                        Some(was) if was != up => self.history.event(
                            if up { "tunnel_up" } else { "tunnel_down" },
                            &info.name,
                            &format!("{id} {}", info.role),
                        ),
                        _ => {}
                    }
                    // The entry side's numbers stand for the tunnel.
                    if info.role == "entry" {
                        let h_ = &self.history;
                        h_.record(
                            &format!("tun:{}:rate", info.name),
                            at,
                            if up { rate.unwrap_or(0.0) } else { 0.0 },
                        );
                        h_.record(
                            &format!("tun:{}:conns", info.name),
                            at,
                            (status.totals.tcp_open + status.totals.udp_flows) as f64,
                        );
                        if let Some(rtt) = status.peer.rtt_ms {
                            h_.record(&format!("tun:{}:rtt", info.name), at, rtt);
                        }
                    }
                }
                TunnelView {
                    info,
                    rate_mbps: rate,
                }
            })
            .collect();
    }

    /// The panel's own server, sampled here (no link).
    pub async fn run_local(self: Arc<Self>) {
        let mut sampler = Sampler::default();
        let mut round = 0u32;
        loop {
            // Old history goes about once an hour.
            if round % 1800 == 0 {
                let _ = self.history.prune();
            }
            round = round.wrapping_add(1);
            let health = sampler.sample();
            let tunnels = collect::tunnels(&self.kariz_dir, &*self.services).await;
            {
                let mut live = self.live();
                let entry = live.entry(LOCAL.to_owned()).or_default();
                entry.online = true;
                entry.hostname = collect::hostname();
                entry.version = env!("CARGO_PKG_VERSION").to_owned();
                entry.arch = std::env::consts::ARCH.to_owned();
            }
            self.update(LOCAL, Some(health), tunnels);
            tokio::time::sleep(POLL).await;
        }
    }

    /// Every server, the panel's own first, for the API.
    pub fn snapshot(&self) -> Result<Vec<ServerView>> {
        let rows: Vec<(String, String, String, String, String)> = {
            let conn = self.db.conn();
            let mut stmt = conn.prepare(
                "SELECT id, name, version, arch, host FROM servers ORDER BY created, id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let live = self.live();
        let view =
            |id: &str, name: String, local: bool, version: String, arch: String, host: String| {
                let l = live.get(id);
                ServerView {
                    id: id.to_owned(),
                    name,
                    local,
                    online: l.is_some_and(|l| l.online),
                    version: l
                        .filter(|l| !l.version.is_empty())
                        .map_or(version, |l| l.version.clone()),
                    arch: l
                        .filter(|l| !l.arch.is_empty())
                        .map_or(arch, |l| l.arch.clone()),
                    hostname: l
                        .filter(|l| !l.hostname.is_empty())
                        .map_or(host, |l| l.hostname.clone()),
                    seen_secs: l.and_then(|l| l.seen).map(|s| s.elapsed().as_secs()),
                    health: l.and_then(|l| l.health.clone()),
                    tunnels: l.map(|l| l.tunnels.clone()).unwrap_or_default(),
                }
            };
        let local_name = live
            .get(LOCAL)
            .map(|l| l.hostname.clone())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(collect::hostname);
        let mut out = vec![view(
            LOCAL,
            local_name,
            true,
            env!("CARGO_PKG_VERSION").to_owned(),
            std::env::consts::ARCH.to_owned(),
            String::new(),
        )];
        for (id, name, version, arch, host) in rows {
            out.push(view(&id, name, false, version, arch, host));
        }
        Ok(out)
    }
}

/// One request to an agent: a stream with the request in its open bytes and the answer
/// in what comes back.
pub async fn request_on(session: &MuxSession, request: &Request) -> Result<Vec<u8>> {
    let syn = serde_json::to_vec(request)?;
    let ask = async {
        let stream = session.open(Bytes::from(syn))?;
        // The request is all in the open bytes: this side has nothing more to send. Left
        // open, the agent's dropping the stream after its answer would reset it.
        stream.finish()?;
        let mut reply = Vec::new();
        while let Some(chunk) = stream.recv().await? {
            reply.extend_from_slice(&chunk);
            if reply.len() > MAX_REPLY {
                bail!("the agent's answer is too large");
            }
        }
        Ok(reply)
    };
    tokio::time::timeout(REQUEST_TIMEOUT, ask)
        .await
        .map_err(|_| anyhow!("the agent did not answer in time"))?
}
