//! Agents ports of their own. Every agent dials the panel's agents port, unless its server
//! was given a port of its own: the panel then listens on that port too (`tcpmux` and `kcp`,
//! TCP and UDP) and on the next one (`wss` and `quic`), for that server's agent alone. A
//! server whose network filters the main agents port can come in on one that gets through,
//! and nothing changes for the other agents.
//!
//! On such a port another agent is let go before anything is written, by the same rule as a
//! link the panel dials ([`Hub::run_session`] with the server it expects). So a server with
//! a port of its own joins with a code for a server the panel lists already (waiting for its
//! agent), never with a code for a new one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use kariz::config::TransportKind;
use kariz::link::Acceptor;
use rusqlite::{params, OptionalExtension};

use crate::hub::{Hub, LOCAL};

/// Where and how the panel listens for agents: set once it listens on its main port.
#[derive(Debug, Clone)]
pub struct AgentListen {
    /// The main agents address (`0.0.0.0:29001`).
    pub listen: String,
    /// The port the panel's web page is served on (an agents port must not take it).
    pub web_port: u16,
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl AgentListen {
    fn main_port(&self) -> Option<u16> {
        self.listen
            .parse::<std::net::SocketAddr>()
            .ok()
            .map(|a| a.port())
    }

    /// The main address with another port.
    fn with_port(&self, port: u16) -> String {
        match self.listen.rsplit_once(':') {
            Some((host, _)) => format!("{host}:{port}"),
            None => format!("0.0.0.0:{port}"),
        }
    }
}

/// The tasks that serve one agents port.
type Listeners = Vec<tokio::task::JoinHandle<()>>;

/// The agents ports the panel listens on now, by server.
#[derive(Default)]
pub struct Ports {
    listen: Mutex<Option<AgentListen>>,
    open: Mutex<HashMap<String, (u16, Listeners)>>,
}

/// Binds every link transport for the agents address `listen`, on each of its addresses
/// (`0.0.0.0` is IPv6 and IPv4). `tcpmux` is what every agent can use: without it this
/// fails; another transport that cannot bind is only logged.
pub async fn bind(listen: &str, token: &str, cert: &Path, key: &Path) -> Result<Vec<Acceptor>> {
    let mut out = Vec::new();
    for kind in kariz::link::LINK_TRANSPORTS {
        let addr = kariz::link::address_for(listen, kind)?;
        let mut bound = 0;
        let mut last = None;
        for one in crate::agent::listen_addresses(&addr) {
            let made = if kind == TransportKind::Wss {
                Acceptor::bind_wss(&one, token, cert, key).await
            } else {
                Acceptor::bind_via(&one, token, kind).await
            };
            match made {
                Ok(acceptor) => {
                    tracing::info!(address = %one, transport = kind.name(), "listening for agents");
                    out.push(acceptor);
                    bound += 1;
                }
                Err(e) => last = Some(e),
            }
        }
        if bound == 0 {
            let e = last.map(|e| e.to_string()).unwrap_or_default();
            if kind == TransportKind::Tcpmux {
                bail!("failed to listen for agents on {addr}: {e}");
            }
            tracing::warn!(address = %addr, transport = kind.name(), error = %e, "agents cannot use this transport");
        }
    }
    Ok(out)
}

/// Whether `port` can be an agents port of its own: it and the next one are free of the
/// panel's page, its main agents ports and the ports of other servers (`port_taken:WHAT`),
/// and the next one exists (`bad_port`).
fn check(listen: &AgentListen, others: &[(String, u16)], server: &str, port: u16) -> Result<()> {
    if port == 0 || port == u16::MAX {
        bail!("bad_port");
    }
    let pair = |p: u16| [p, p.saturating_add(1)];
    let clashes = |taken: u16| pair(port).iter().any(|p| pair(taken).contains(p));
    if pair(port).contains(&listen.web_port) {
        bail!("port_taken:panel");
    }
    if listen.main_port().is_some_and(clashes) {
        bail!("port_taken:agents");
    }
    if let Some((other, _)) = others.iter().find(|(s, p)| s != server && clashes(*p)) {
        bail!("port_taken:{other}");
    }
    Ok(())
}

/// Stops listeners and waits until they have let their sockets go.
async fn stopped(handles: Listeners) {
    for handle in &handles {
        handle.abort();
    }
    for handle in handles {
        let _ = handle.await;
    }
}

impl Hub {
    /// Remembers where the panel listens for agents (it does once it starts).
    pub fn set_agent_listen(&self, listen: AgentListen) {
        *self.ports.listen.lock().unwrap_or_else(|e| e.into_inner()) = Some(listen);
    }

    /// The agents port of `server`'s own, if it has one.
    pub fn agent_port_of(&self, server: &str) -> Option<u16> {
        self.db
            .conn()
            .query_row(
                "SELECT port FROM agent_ports WHERE server = ?1",
                [server],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn agent_ports(&self) -> Result<Vec<(String, u16)>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT server, port FROM agent_ports ORDER BY server")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Gives `server` an agents port of its own (`Some`), or takes it away (`None`: its
    /// agent dials the main port again). The new listeners are bound before anything else
    /// changes, so a port that is in use fails (`port_busy:WHY`) and leaves the old one.
    /// Errors also: `agents_off`, `bad_port`, `port_taken:WHAT`, `no_such_server`.
    pub async fn set_agent_port(self: &Arc<Self>, server: &str, port: Option<u16>) -> Result<()> {
        let Some(port) = port else {
            // Its sockets are let go before this returns, so the same port can be given again
            // at once.
            if let Some(handles) = self.take_agent_port(server) {
                stopped(handles).await;
            }
            self.db
                .conn()
                .execute("DELETE FROM agent_ports WHERE server = ?1", [server])?;
            return Ok(());
        };
        if server == LOCAL || !self.knows(server)? {
            bail!("no_such_server");
        }
        let listen = self
            .ports
            .listen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow!("agents_off"))?;
        if listen.main_port() == Some(port) {
            // The main port: no port of its own.
            return Box::pin(self.set_agent_port(server, None)).await;
        }
        check(&listen, &self.agent_ports()?, server, port)?;
        let already = self
            .ports
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(server)
            .is_some_and(|(p, _)| *p == port);
        if !already {
            self.open_agent_port(&listen, server, port).await?;
        }
        self.db.conn().execute(
            "INSERT INTO agent_ports (server, port) VALUES (?1, ?2)
             ON CONFLICT (server) DO UPDATE SET port = excluded.port",
            params![server, port],
        )?;
        Ok(())
    }

    /// Binds `port` for `server`'s agent and serves it, in place of a port it had.
    async fn open_agent_port(
        self: &Arc<Self>,
        listen: &AgentListen,
        server: &str,
        port: u16,
    ) -> Result<()> {
        let token = self.link_token()?;
        let acceptors = bind(&listen.with_port(port), &token, &listen.cert, &listen.key)
            .await
            .map_err(|e| anyhow!("port_busy:{e:#}"))?;
        let handles = acceptors
            .into_iter()
            .map(|a| tokio::spawn(self.clone().serve_agents_as(a, Some(server.to_owned()))))
            .collect();
        let old = self
            .ports
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server.to_owned(), (port, handles));
        if let Some((_, old)) = old {
            stopped(old).await;
        }
        tracing::info!(server, port, "listening for one agent on a port of its own");
        Ok(())
    }

    /// Stops listening on `server`'s own agents port (its connected agent stays connected
    /// until its link ends).
    pub fn close_agent_port(&self, server: &str) {
        for handle in self.take_agent_port(server).unwrap_or_default() {
            handle.abort();
        }
    }

    fn take_agent_port(&self, server: &str) -> Option<Listeners> {
        self.ports
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(server)
            .map(|(_, handles)| handles)
    }

    /// Listens on every saved agents port of a server's own (the panel's start, a restored
    /// backup). One that cannot be bound is logged and left: that server's agent cannot come
    /// in until the port is free or changed.
    pub async fn open_saved_agent_ports(self: &Arc<Self>) {
        let Some(listen) = self
            .ports
            .listen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return;
        };
        for (server, port) in self.agent_ports().unwrap_or_default() {
            if let Err(e) = self.open_agent_port(&listen, &server, port).await {
                tracing::warn!(server, port, error = %e, "a server's own agents port cannot be opened");
            }
        }
    }

    fn knows(&self, server: &str) -> Result<bool> {
        Ok(self
            .db
            .conn()
            .query_row("SELECT 1 FROM servers WHERE id = ?1", [server], |_| Ok(()))
            .optional()?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen() -> AgentListen {
        AgentListen {
            listen: "0.0.0.0:29001".into(),
            web_port: 28443,
            cert: PathBuf::new(),
            key: PathBuf::new(),
        }
    }

    fn why(r: Result<()>) -> String {
        format!("{:#}", r.unwrap_err())
    }

    #[test]
    fn a_port_of_its_own_and_the_next_are_free_of_every_other() {
        let others = vec![("far".to_owned(), 8443u16)];
        assert!(check(&listen(), &others, "me", 443).is_ok());
        assert!(check(&listen(), &others, "me", 2053).is_ok());
        // the panel's page, and the port before it (whose next one it is)
        assert_eq!(
            why(check(&listen(), &others, "me", 28443)),
            "port_taken:panel"
        );
        assert_eq!(
            why(check(&listen(), &others, "me", 28442)),
            "port_taken:panel"
        );
        // the main agents ports 29001 and 29002
        for p in [29000, 29001, 29002] {
            assert_eq!(
                why(check(&listen(), &others, "me", p)),
                "port_taken:agents",
                "{p}"
            );
        }
        assert!(check(&listen(), &others, "me", 29003).is_ok());
        // another server's 8443 and 8444
        for p in [8442, 8443, 8444] {
            assert_eq!(
                why(check(&listen(), &others, "me", p)),
                "port_taken:far",
                "{p}"
            );
        }
        // its own port again is fine
        assert!(check(&listen(), &others, "far", 8443).is_ok());
        // the next port must exist
        assert_eq!(why(check(&listen(), &others, "me", 65535)), "bad_port");
        assert_eq!(why(check(&listen(), &others, "me", 0)), "bad_port");
    }

    /// Starts an agent from a join code, its `ip` and `ping` real (it makes no links here).
    fn start_agent(dir: &Path, code: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join("agent.toml");
        let config = crate::agent::enroll_from_code(code, &file).unwrap();
        config.save(&file).unwrap();
        let agent = crate::agent::Agent::with_exec(
            &file,
            config,
            Arc::new(crate::manage::Systemd),
            Arc::new(crate::net::Real),
        );
        tokio::spawn(agent.run());
    }

    #[tokio::test]
    async fn an_agent_comes_in_on_its_own_port_and_no_other_agent_does() {
        let root = tempfile::tempdir().unwrap();
        let hub = Hub::new(
            crate::db::Db::in_memory().unwrap(),
            root.path().join("kariz"),
        );
        let main = Acceptor::bind("127.0.0.1:0", &hub.link_token().unwrap())
            .await
            .unwrap();
        let main_port = main.local_addr().unwrap().port();
        tokio::spawn(hub.clone().serve_agents(main));
        let (id, name) = hub.add_waiting(Some("far"), "").unwrap();
        assert_eq!(name, "far");

        // Without the panel listening for agents there is nothing to add a port to.
        assert_eq!(
            why(hub.set_agent_port(&id, Some(40000)).await),
            "agents_off"
        );
        hub.set_agent_listen(AgentListen {
            listen: format!("127.0.0.1:{main_port}"),
            web_port: 1,
            cert: root.path().join("none.pem"),
            key: root.path().join("none.key"),
        });
        // A port (and the next) that nothing else holds.
        let mut own = 0;
        for _ in 0..20 {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = probe.local_addr().unwrap().port();
            drop(probe);
            if p < u16::MAX && hub.set_agent_port(&id, Some(p)).await.is_ok() {
                own = p;
                break;
            }
        }
        assert_ne!(own, 0, "a free port was found");
        assert_eq!(hub.agent_port_of(&id), Some(own));
        // The main port itself is no port of its own: asking for it clears the one it had.
        hub.set_agent_port(&id, Some(main_port)).await.unwrap();
        assert_eq!(hub.agent_port_of(&id), None);
        hub.set_agent_port(&id, Some(own)).await.unwrap();

        // The server's agent, with its code, comes in on its own port.
        let code = hub
            .rejoin_code(&id, &format!("127.0.0.1:{own}"), Some("tcpmux"), None, None)
            .unwrap();
        start_agent(&root.path().join("far"), &code);
        let online = |hub: &Arc<Hub>, id: &str| {
            hub.snapshot()
                .unwrap()
                .into_iter()
                .any(|s| s.id == id && s.online)
        };
        for _ in 0..100 {
            if online(&hub, &id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(online(&hub, &id), "the agent came in on its own port");
        let view = hub
            .snapshot()
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        assert_eq!(view.agent_port, Some(own));

        // An agent with a code for a new server is let go there.
        let before = hub.snapshot().unwrap().len();
        let stranger = hub
            .create_join_via(
                Some("stranger"),
                &format!("127.0.0.1:{own}"),
                Some("tcpmux"),
            )
            .unwrap();
        start_agent(&root.path().join("stranger"), &stranger);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let after = hub.snapshot().unwrap();
        assert_eq!(after.len(), before, "no server was added on another's port");
        assert!(!after.iter().any(|s| s.name == "stranger"));

        // Removing the server closes its port.
        assert!(hub.remove(&id).unwrap());
        assert_eq!(hub.agent_port_of(&id), None);
        let mut closed = false;
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(("127.0.0.1", own))
                .await
                .is_err()
            {
                closed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(closed, "nothing listens on its port any more");
    }

    #[test]
    fn the_main_address_keeps_its_host() {
        assert_eq!(listen().with_port(443), "0.0.0.0:443");
        let v6 = AgentListen {
            listen: "[::]:29001".into(),
            ..listen()
        };
        assert_eq!(v6.with_port(443), "[::]:443");
    }
}
