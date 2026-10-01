//! The panel's side of the agents: it accepts their links, checks who they are, asks them
//! for their health and tunnels every couple of seconds, and keeps the latest of it for
//! the API.
//!
//! An agent proves itself in one of two ways. A registered one answers the panel's
//! random challenge with a keyed hash only its key makes. A new one shows the join
//! secret from its join code, which works once; the panel then makes it an identity (an
//! id and a key), sends it over the link, and forgets the secret. The link token in the
//! handshake is the first gate; the key is the second, so a leaked token alone opens no
//! server.

use std::collections::HashMap;
use std::path::PathBuf;
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
/// How many times in a row the list of tunnels may fail before the link is given up.
const LATE_TUNNELS: u32 = 3;

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
    /// The address other servers reach this one at (for private networks), if set.
    pub addr: Option<String>,
    pub seen_secs: Option<u64>,
    /// The transport its agent's link uses now (`tcpmux` or `kcp`); none for the panel's
    /// own server and for one that is offline.
    pub link: Option<String>,
    pub health: Option<Health>,
    /// The server's public IPv4 and IPv6 addresses, when they are known: what its agent
    /// reports, or the address its link comes from.
    pub ip4: Option<String>,
    pub ip6: Option<String>,
    pub tunnels: Vec<TunnelView>,
}

/// The public addresses of a server: its own (as its health reports them) when they are
/// public, else the address its link was seen coming from.
fn addresses(
    health: Option<&Health>,
    peer: Option<std::net::IpAddr>,
) -> (Option<String>, Option<String>) {
    let mut v4 = health.and_then(|h| h.ip4.clone());
    let mut v6 = health.and_then(|h| h.ip6.clone());
    if let Some(ip) = peer {
        let text = ip.to_string();
        match ip {
            std::net::IpAddr::V4(_)
                if collect::public_ip4(&text)
                    && !v4.as_deref().is_some_and(collect::public_ip4) =>
            {
                v4 = Some(text);
            }
            std::net::IpAddr::V6(_)
                if collect::public_ip6(&text)
                    && !v6.as_deref().is_some_and(collect::public_ip6) =>
            {
                v6 = Some(text);
            }
            _ => {}
        }
    }
    (v4, v6)
}

/// What a join secret is good for.
enum Spent {
    /// A new server, with the name the code carried (empty: the agent's host name).
    New(String),
    /// The server it registered before, whose agent has not confirmed yet.
    Unconfirmed { id: String, key: String },
}

#[derive(Default)]
struct Live {
    online: bool,
    seen: Option<Instant>,
    hostname: String,
    version: String,
    arch: String,
    link: Option<&'static str>,
    health: Option<Health>,
    /// Where the last link of this server came from.
    peer_ip: Option<std::net::IpAddr>,
    tunnels: Vec<TunnelView>,
    /// Bytes counted at the last reading of each tunnel, for the rates.
    last_bytes: HashMap<String, (u64, Instant)>,
    session: Option<Arc<MuxSession>>,
    /// Whether each tunnel was connected at the last reading, to notice it changing.
    connected: HashMap<String, bool>,
}

pub struct Hub {
    pub(crate) db: Db,
    kariz_dir: PathBuf,
    live: Mutex<HashMap<String, Live>>,
    /// The panel's own server, which answers the same requests as an agent, in-process.
    local: Arc<Agent>,
    services: Arc<dyn Services>,
    /// The running and recent tunnel operations (`crate::pair`).
    pub ops: crate::pair::Ops,
    /// Charts and events.
    pub history: crate::history::History,
    /// Private networks and their links.
    pub networks: crate::networks::Networks,
    /// How long a new tunnel has to connect (shortened in tests).
    pub connect_wait: Duration,
    /// Where the panel finds releases and its own files (set once the panel starts).
    pub update_settings: std::sync::OnceLock<crate::updater::UpdateSettings>,
    /// What the last check for a newer release found.
    pub checked: Mutex<crate::updater::Checked>,
}

pub const LOCAL: &str = "local";

impl Hub {
    pub fn new(db: Db, kariz_dir: PathBuf) -> Arc<Self> {
        Self::with_services(db, kariz_dir, Arc::new(Systemd))
    }

    /// A hub whose own server runs its tunnels through `services`.
    pub fn with_services(db: Db, kariz_dir: PathBuf, services: Arc<dyn Services>) -> Arc<Self> {
        Self::with_options(
            db,
            kariz_dir,
            services,
            crate::pair::CONNECT_WAIT,
            None,
            None,
        )
    }

    /// Like [`Hub::with_services`], and this server's private network links are kept in
    /// `state_dir`, so they come back after a reboot.
    pub fn with_state_dir(
        db: Db,
        kariz_dir: PathBuf,
        services: Arc<dyn Services>,
        state_dir: PathBuf,
    ) -> Arc<Self> {
        Self::with_options(
            db,
            kariz_dir,
            services,
            crate::pair::CONNECT_WAIT,
            Some(state_dir),
            None,
        )
    }

    /// Also sets how long a new tunnel has to connect.
    pub fn with_options(
        db: Db,
        kariz_dir: PathBuf,
        services: Arc<dyn Services>,
        connect_wait: Duration,
        state_dir: Option<PathBuf>,
        exec: Option<Arc<dyn crate::net::Exec>>,
    ) -> Arc<Self> {
        let config = AgentConfig {
            panel: String::new(),
            link_token: String::new(),
            join: None,
            id: None,
            key: None,
            kariz_dir: kariz_dir.clone(),
            services: Default::default(),
            release_key: None,
            transport: None,
        };
        Arc::new(Self {
            history: crate::history::History::new(db.clone()),
            networks: crate::networks::Networks::new(db.clone()),
            db,
            local: Agent::with_exec(
                &state_dir.map_or_else(PathBuf::new, |d| d.join("local-agent.toml")),
                config,
                services.clone(),
                exec.unwrap_or_else(|| Arc::new(crate::net::Real)),
            ),
            services,
            ops: crate::pair::Ops::default(),
            connect_wait,
            update_settings: std::sync::OnceLock::new(),
            checked: Mutex::new(crate::updater::Checked::default()),
            kariz_dir,
            live: Mutex::new(HashMap::new()),
        })
    }

    /// One request to a server (the panel's own, or an agent that is connected), and its
    /// raw answer.
    pub async fn ask(&self, server: &str, request: &Request) -> Result<Vec<u8>> {
        self.ask_within(server, request, REQUEST_TIMEOUT).await
    }

    /// Like [`Hub::ask`], for a request that is allowed to take up to `limit`.
    pub async fn ask_within(
        &self,
        server: &str,
        request: &Request,
        limit: Duration,
    ) -> Result<Vec<u8>> {
        if server == LOCAL {
            return Ok(self.local.handle(request.clone()).await);
        }
        let session = self
            .live()
            .get(server)
            .filter(|l| l.online)
            .and_then(|l| l.session.clone())
            .ok_or_else(|| anyhow!("that server is not connected"))?;
        request_within(&session, request, limit).await
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

    /// The address other servers reach `server` at, if it has been set.
    pub fn addr_of(&self, server: &str) -> Option<String> {
        self.db
            .conn()
            .query_row(
                "SELECT addr FROM server_addrs WHERE server = ?1",
                [server],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Sets (or with an empty address, clears) the address other servers reach `server`
    /// at. A server that is not known is refused.
    pub fn set_addr(&self, server: &str, addr: &str) -> Result<()> {
        if server != LOCAL
            && self
                .db
                .conn()
                .query_row("SELECT 1 FROM servers WHERE id = ?1", [server], |_| Ok(()))
                .optional()?
                .is_none()
        {
            bail!("no_such_server");
        }
        let addr = addr.trim();
        if addr.is_empty() {
            self.db
                .conn()
                .execute("DELETE FROM server_addrs WHERE server = ?1", [server])?;
            return Ok(());
        }
        if !crate::netops::valid_addr(addr) {
            bail!("bad_input");
        }
        self.db.conn().execute(
            "INSERT INTO server_addrs (server, addr) VALUES (?1, ?2)
             ON CONFLICT (server) DO UPDATE SET addr = excluded.addr",
            params![server, addr],
        )?;
        Ok(())
    }

    /// Whether a server can be asked right now (the panel's own always can).
    pub fn is_online(&self, server: &str) -> bool {
        server == LOCAL || self.live().get(server).is_some_and(|l| l.online)
    }

    /// Remembers that a tunnel is to be removed from a server that cannot be reached now:
    /// its agent does it when it connects again. The tunnel disappears from the lists at once.
    pub fn queue_delete(&self, server: &str, name: &str) -> Result<()> {
        self.db.conn().execute(
            "INSERT OR IGNORE INTO pending_deletes (server, name) VALUES (?1, ?2)",
            params![server, name],
        )?;
        Ok(())
    }

    /// The names waiting to be removed from `server`.
    fn pending_deletes(&self, server: &str) -> Vec<String> {
        let conn = self.db.conn();
        let Ok(mut stmt) = conn.prepare("SELECT name FROM pending_deletes WHERE server = ?1")
        else {
            return Vec::new();
        };
        stmt.query_map([server], |r| r.get(0))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Whether a tunnel of this name is waiting to be removed from some server (the name
    /// cannot be used again until it is gone).
    pub fn delete_pending_for(&self, name: &str) -> bool {
        self.db
            .conn()
            .query_row(
                "SELECT 1 FROM pending_deletes WHERE name = ?1",
                [name],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Removes the tunnels that were deleted while this server was away.
    async fn apply_pending_deletes(&self, id: &str) {
        for name in self.pending_deletes(id) {
            let done = self
                .ask_as::<Ack>(id, &Request::TunnelDelete { name: name.clone() })
                .await;
            match done {
                Ok(ack) if ack.ok => {
                    let _ = self.db.conn().execute(
                        "DELETE FROM pending_deletes WHERE server = ?1 AND name = ?2",
                        params![id, name],
                    );
                    info!(server = %id, tunnel = %name, "a tunnel deleted while the server was away is removed");
                }
                Ok(ack) => {
                    warn!(server = %id, tunnel = %name, error = ?ack.error, "could not remove a deleted tunnel")
                }
                Err(e) => {
                    warn!(server = %id, tunnel = %name, error = %e, "could not remove a deleted tunnel")
                }
            }
        }
    }

    /// The networks each connected server already routes, by server id.
    pub fn routes(&self) -> crate::networks::Routes {
        self.live()
            .iter()
            .filter_map(|(id, l)| Some((id.clone(), l.health.as_ref()?.routes.clone())))
            .collect()
    }

    /// A join code for a new server: `name` (optional) is what it will be called, and
    /// `panel` where it should dial.
    pub fn create_join(&self, name: Option<&str>, panel: &str) -> Result<String> {
        self.create_join_via(name, panel, None)
    }

    /// Like [`Hub::create_join`], for an agent that is to use only `transport`.
    pub fn create_join_via(
        &self,
        name: Option<&str>,
        panel: &str,
        transport: Option<&str>,
    ) -> Result<String> {
        if transport.is_some_and(|x| !join::valid_transport(x)) {
            bail!("unknown transport");
        }
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
            x: transport.map(str::to_owned),
        }))
    }

    /// Spends a join secret, if it is valid: a new server to register (with the name the
    /// code carried), or the server it already registered whose agent never confirmed.
    fn spend_join(&self, secret: &str) -> Result<Option<Spent>> {
        let conn = self.db.conn();
        let t = now();
        conn.execute("DELETE FROM joins WHERE expires < ?1 - 86400", [t])?;
        let hash = hash_token(secret);
        let row: Option<(String, bool, Option<String>)> = conn
            .query_row(
                "SELECT name, used, server FROM joins WHERE hash = ?1 AND expires > ?2",
                params![hash, t],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((name, false, _)) => {
                let changed = conn.execute(
                    "UPDATE joins SET used = 1 WHERE hash = ?1 AND used = 0",
                    [&hash],
                )?;
                Ok((changed == 1).then_some(Spent::New(name)))
            }
            Some((_, true, Some(id))) => {
                let key: Option<String> = conn
                    .query_row("SELECT key FROM servers WHERE id = ?1", [&id], |r| r.get(0))
                    .optional()?;
                Ok(key.map(|key| Spent::Unconfirmed { id, key }))
            }
            Some((_, true, None)) => Ok(None),
        }
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
    pub fn remove(self: &Arc<Self>, id: &str) -> Result<bool> {
        if id == LOCAL {
            return Ok(false);
        }
        // Its private network links go with it; the servers on their other ends are
        // told at once.
        let peers: Vec<String> = self
            .networks
            .links(None)?
            .into_iter()
            .filter(|l| l.a == id || l.b == id)
            .map(|l| if l.a == id { l.b } else { l.a })
            .collect();
        {
            let conn = self.db.conn();
            conn.execute("DELETE FROM net_links WHERE a = ?1 OR b = ?1", [id])?;
            conn.execute("DELETE FROM server_addrs WHERE server = ?1", [id])?;
            conn.execute("DELETE FROM pending_deletes WHERE server = ?1", [id])?;
        }
        let removed = self
            .db
            .conn()
            .execute("DELETE FROM servers WHERE id = ?1", [id])?
            == 1;
        for peer in peers {
            let hub = self.clone();
            tokio::spawn(async move {
                let _ = hub.net_sync(&peer).await;
            });
        }
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
        let link = acceptor.kind().name();
        loop {
            match acceptor.accept().await {
                Ok(pending) => {
                    let hub = self.clone();
                    let peer = pending.peer();
                    tokio::spawn(async move {
                        match pending.establish(Side::Client).await {
                            Ok(session) => {
                                let ip = peer.ip().to_canonical();
                                if let Err(e) = hub.run_session(Arc::new(session), link, ip).await {
                                    debug!(%peer, error = %e, "an agent session ended");
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

    async fn run_session(
        &self,
        session: Arc<MuxSession>,
        link: &'static str,
        peer_ip: std::net::IpAddr,
    ) -> Result<()> {
        let identified = tokio::time::timeout(Duration::from_secs(20), self.identify(&session))
            .await
            .map_err(|_| anyhow!("the agent did not identify itself in time"))
            .and_then(|r| r);
        let (id, hello) = match identified {
            Ok(found) => found,
            Err(e) => {
                // Said at warn: this is why a new server never shows up.
                warn!(error = %format!("{e:#}"), "an agent could not be identified");
                return Err(e);
            }
        };
        info!(server = %id, "an agent is connected");
        {
            let mut live = self.live();
            let entry = live.entry(id.clone()).or_default();
            // A new link replaces an old one that has not noticed it is dead.
            if let Some(old) = entry.session.replace(session.clone()) {
                old.close();
            }
            entry.online = true;
            entry.link = Some(link);
            entry.peer_ip = Some(peer_ip);
            entry.hostname = hello.hostname.clone();
            entry.version = hello.version.clone();
            entry.arch = hello.arch.clone();
        }
        self.history.event("server_up", &id, "");
        self.apply_pending_deletes(&id).await;
        // The server is told its private network links (they may have changed while it
        // was away).
        if let Err(e) = self.net_sync(&id).await {
            warn!(server = %id, error = %e, "the private network links could not be synced");
        }
        let result = self.poll(&id, &session).await;
        // Said at warn, with the reason: when a server goes offline by itself, this is the
        // line that tells why (the agent's own log has the other end of it).
        if let Err(e) = &result {
            warn!(server = %id, transport = link, error = %format!("{e:#}"), "an agent link ended");
        }
        // Closed from this side too, so the agent sees it end at once and comes back.
        session.close();
        let mut live = self.live();
        if let Some(entry) = live.get_mut(&id) {
            // Only if this link is still the current one.
            if entry
                .session
                .as_ref()
                .is_some_and(|s| Arc::ptr_eq(s, &session))
            {
                entry.online = false;
                entry.link = None;
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
        let Some(spent) = self.spend_join(secret)? else {
            bail!("an agent with a join secret that is wrong, used or expired");
        };
        let hash = hash_token(secret);
        let (id, key, name) = match spent {
            Spent::New(wanted) => {
                let wanted = if wanted.is_empty() {
                    hello.hostname.clone()
                } else {
                    wanted
                };
                let (id, key, name) = self.register(&wanted, &hello)?;
                self.db.conn().execute(
                    "UPDATE joins SET server = ?2 WHERE hash = ?1",
                    params![hash, id],
                )?;
                (id, key, name)
            }
            // Registered on an earlier link that dropped before the agent answered: the
            // same identity again (the agent may or may not have kept it; if it has, it
            // comes with its proof instead and never gets here).
            Spent::Unconfirmed { id, key } => {
                let name: String = self.db.conn().query_row(
                    "SELECT name FROM servers WHERE id = ?1",
                    [&id],
                    |r| r.get(0),
                )?;
                (id, key, name)
            }
        };
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
            bail!(
                "the agent did not keep its identity: {}",
                ack.error.unwrap_or_default()
            );
        }
        // Confirmed: the code can never give this identity out again.
        self.db
            .conn()
            .execute("UPDATE joins SET server = NULL WHERE hash = ?1", [&hash])?;
        let _ = self
            .db
            .audit("hub", None, &format!("registered server {name} ({id})"));
        Ok((id, hello))
    }

    /// Asks the agent for its state every [`POLL`] until the link ends.
    async fn poll(&self, id: &str, session: &Arc<MuxSession>) -> Result<()> {
        let mut tunnels: Vec<TunnelInfo> = Vec::new();
        let mut late = 0u32;
        loop {
            let health: Health =
                serde_json::from_slice(&request_on(session, &Request::Health).await?)?;
            // The list of tunnels is the slow request (every daemon is asked). One that
            // comes late is no reason to drop a link that answers everything else: the
            // last list stays, and only a few in a row end the link.
            match request_on(session, &Request::Tunnels).await {
                Ok(raw) => {
                    tunnels = serde_json::from_slice(&raw)?;
                    late = 0;
                }
                Err(e) => {
                    late += 1;
                    if late >= LATE_TUNNELS {
                        return Err(e);
                    }
                    debug!(server = %id, error = %e, "the tunnel list came late");
                }
            }
            self.update(id, Some(health), tunnels.clone());
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
        self.local.restore_net().await;
        if let Err(e) = self.net_sync(LOCAL).await {
            warn!(error = %e, "the private network links could not be synced");
        }
        let mut sampler = Sampler::default();
        let mut round = 0u32;
        loop {
            // Old history goes about once an hour.
            if round % 1800 == 0 {
                let _ = self.history.prune();
            }
            // A look for a newer release: a minute after the start, then every day. It
            // only looks; installing is always the user's button.
            if (round == 30 || round % 43_200 == 43_199) && self.update_auto() {
                let _ = self.check_update().await;
            }
            round = round.wrapping_add(1);
            let health = sampler.sample();
            let tunnels = collect::tunnels(&self.kariz_dir, &*self.services).await;
            {
                let mut live = self.live();
                let entry = live.entry(LOCAL.to_owned()).or_default();
                entry.online = true;
                entry.hostname = collect::hostname();
                entry.version = crate::version().to_owned();
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
                let hidden = self.pending_deletes(id);
                let (ip4, ip6) =
                    addresses(l.and_then(|l| l.health.as_ref()), l.and_then(|l| l.peer_ip));
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
                    addr: None,
                    seen_secs: l.and_then(|l| l.seen).map(|s| s.elapsed().as_secs()),
                    link: l
                        .filter(|l| l.online)
                        .and_then(|l| l.link)
                        .map(str::to_owned),
                    health: l.and_then(|l| l.health.clone()),
                    ip4,
                    ip6,
                    tunnels: l
                        .map(|l| {
                            l.tunnels
                                .iter()
                                .filter(|t| !hidden.contains(&t.info.name))
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
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
            crate::version().to_owned(),
            std::env::consts::ARCH.to_owned(),
            String::new(),
        )];
        for (id, name, version, arch, host) in rows {
            out.push(view(&id, name, false, version, arch, host));
        }
        let addrs: HashMap<String, String> = {
            let conn = self.db.conn();
            let mut stmt = conn.prepare("SELECT server, addr FROM server_addrs")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for s in &mut out {
            s.addr = addrs.get(&s.id).cloned();
        }
        Ok(out)
    }
}

/// One request to an agent: a stream with the request in its open bytes and the answer
/// in what comes back.
pub async fn request_on(session: &MuxSession, request: &Request) -> Result<Vec<u8>> {
    request_within(session, request, REQUEST_TIMEOUT).await
}

/// Like [`request_on`], with its own time limit (a speed test takes a while).
pub async fn request_within(
    session: &MuxSession,
    request: &Request,
    limit: Duration,
) -> Result<Vec<u8>> {
    let syn = serde_json::to_vec(request)?;
    if syn.len() > crate::wire::MAX_REQUEST {
        bail!("too_large");
    }
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
    tokio::time::timeout(limit, ask)
        .await
        .map_err(|_| anyhow!("the agent did not answer in time"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(ip4: Option<&str>, ip6: Option<&str>) -> Health {
        Health {
            ip4: ip4.map(str::to_owned),
            ip6: ip6.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn a_delete_for_a_server_that_is_away_is_kept_and_hides_the_tunnel() {
        let hub = Hub::new(
            Db::in_memory().unwrap(),
            std::env::temp_dir().join("kariz-none"),
        );
        assert!(hub.is_online(LOCAL) && !hub.is_online("a1"));
        assert!(!hub.delete_pending_for("main"));
        hub.queue_delete("a1", "main").unwrap();
        hub.queue_delete("a1", "main").unwrap();
        assert!(hub.delete_pending_for("main") && !hub.delete_pending_for("other"));
        assert_eq!(hub.pending_deletes("a1"), vec!["main".to_owned()]);
        assert!(hub.pending_deletes("a2").is_empty());
    }

    #[test]
    fn a_servers_public_addresses_are_its_own_or_the_ones_its_link_comes_from() {
        let v4 = |s: &str| Some(std::net::IpAddr::V4(s.parse().unwrap()));
        // Its own public address is kept.
        let h = health(Some("5.9.10.11"), Some("2a01:4f8::1"));
        assert_eq!(
            addresses(Some(&h), v4("203.0.113.9")),
            (Some("5.9.10.11".into()), Some("2a01:4f8::1".into()))
        );
        // Behind NAT (a private address), the address the link is seen from stands in.
        let h = health(Some("10.0.0.5"), None);
        assert_eq!(
            addresses(Some(&h), v4("203.0.113.9")),
            (Some("203.0.113.9".into()), None)
        );
        // A private peer (a test, a private network) says nothing.
        assert_eq!(addresses(None, v4("192.168.1.4")), (None, None));
        // No health yet: only the peer.
        assert_eq!(
            addresses(None, v4("203.0.113.9")),
            (Some("203.0.113.9".into()), None)
        );
    }
}
