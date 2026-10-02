//! A tunnel is a pair: the entry on one server and the exit on another. This makes, edits,
//! controls and deletes both sides as one operation. Each
//! operation runs in the background as a list of steps that the browser follows, and a
//! step that fails undoes what the earlier ones did.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::random_hex;
use crate::hub::Hub;
use crate::manage::valid_name;
use crate::wire::{
    Ack, CheckReply, ForwardInfo, PutReply, Request, Spec, SpeedReply, TextReply, TunnelInfo,
};

/// How long a new tunnel has to connect before it counts as failed.
pub const CONNECT_WAIT: Duration = Duration::from_secs(30);

/// What the wizard sends: one tunnel, both sides.
#[derive(Debug, Clone, Deserialize)]
pub struct PairRequest {
    pub name: String,
    /// Server ids (`local` for the panel's own).
    pub entry: String,
    pub exit: String,
    pub mode: String,
    pub transport: String,
    pub profile: Option<String>,
    /// The address the accepting side listens on, e.g. `0.0.0.0:3080`.
    pub listen: String,
    /// The address the dialing side connects to: the accepting server's, as the other sees it.
    pub dial: String,
    pub pool: Option<u32>,
    pub ws_path: Option<String>,
    pub ws_host: Option<String>,
    pub tls_sni: Option<String>,
    /// Mux settings, the same on both sides; left out, the profile's apply.
    #[serde(default)]
    pub mux: Option<crate::wire::MuxSpec>,
    /// `auto` (or left out), `aes-256-gcm`, `chacha20-poly1305` or `none`: the same on both
    /// sides (a side that is not `auto` refuses a peer that uses another cipher).
    #[serde(default)]
    pub encryption: Option<String>,
    /// quic: seal every UDP packet with a key from the token (`[tunnel.quic] obfs`), the same
    /// on both sides. Ignored for other transports.
    #[serde(default)]
    pub quic_obfs: bool,
    /// wss: the files of a real certificate on the listening side (from [`certificate`]),
    /// instead of a self-signed one the dialing side pins.
    #[serde(default)]
    pub tls_cert: Option<String>,
    #[serde(default)]
    pub tls_key: Option<String>,
    #[serde(default)]
    pub forwards: Vec<ForwardInfo>,
    /// Edit only: make a new token for both sides.
    #[serde(default)]
    pub rotate: bool,
    /// Direct mode only: run the tunnel over a private GRE network (its id). The panel
    /// finds or makes the link between the two servers and the tunnel listens on and
    /// dials the private address.
    #[serde(default)]
    pub network: Option<String>,
}

impl PairRequest {
    /// The two specs. `token` is `None` on an edit that keeps the token; `pin` is the
    /// listening wss side's certificate pin, for the dialing side.
    pub fn specs(&self, token: Option<&str>, pin: Option<&str>) -> (Spec, Spec) {
        let reverse = self.mode == "reverse";
        let side = |role: &str, accepts: bool| Spec {
            name: self.name.clone(),
            role: role.to_owned(),
            mode: self.mode.clone(),
            transport: self.transport.clone(),
            profile: self.profile.clone(),
            listen: accepts.then(|| self.listen.clone()),
            remote: (!accepts).then(|| self.dial.clone()),
            token: token.map(str::to_owned),
            pool: self.pool,
            ws_path: self.ws_path.clone(),
            ws_host: self.ws_host.clone(),
            mux: self.mux.clone().filter(|m| !m.is_empty()),
            encryption: self.chosen_encryption().map(str::to_owned),
            quic_obfs: self.sealed(),
            tls_sni: (!accepts).then(|| self.tls_sni.clone()).flatten(),
            tls_pin: (!accepts && self.transport == "wss" && self.tls_cert.is_none())
                .then(|| pin.map(str::to_owned))
                .flatten(),
            tls_cert: (accepts && self.transport == "wss")
                .then(|| self.tls_cert.clone())
                .flatten(),
            tls_key: (accepts && self.transport == "wss")
                .then(|| self.tls_key.clone())
                .flatten(),
            forwards: if role == "entry" {
                self.forwards.clone()
            } else {
                Vec::new()
            },
        };
        (side("entry", reverse), side("exit", !reverse))
    }

    /// The server that accepts the tunnel's connections, and the one that dials.
    pub fn acceptor_first(&self) -> [&str; 2] {
        if self.mode == "reverse" {
            [&self.entry, &self.exit]
        } else {
            [&self.exit, &self.entry]
        }
    }

    /// Whether this tunnel uses something older servers do not know (`auto`, mux settings):
    /// both of its servers must run 1.5 or newer, or they would refuse the config.
    fn needs_new_servers(&self) -> bool {
        self.transport == "auto" || self.mux.as_ref().is_some_and(|m| !m.is_empty())
    }

    /// Whether the tunnel's QUIC packets are sealed.
    fn sealed(&self) -> bool {
        self.quic_obfs && self.transport == "quic"
    }

    /// The cipher to write: `None` for `auto`.
    fn chosen_encryption(&self) -> Option<&str> {
        self.encryption.as_deref().filter(|e| *e != "auto")
    }

    /// An error (`old_agent:SERVER:VERSION`, or `old_agent_enc:...` for a cipher and
    /// `old_agent_obfs:...` for sealed QUIC) naming the
    /// first server that is too old.
    fn require_new_enough(&self, hub: &Hub) -> Result<()> {
        let enc = self.chosen_encryption().is_some();
        let obfs = self.sealed();
        if !self.needs_new_servers() && !enc && !obfs {
            return Ok(());
        }
        for server in hub.snapshot()? {
            if server.id != self.entry && server.id != self.exit {
                continue;
            }
            // A cipher setting came after 1.5.2; auto and mux settings with 1.5.0.
            if enc && !version_at_least(&server.version, (1, 5, 3)) {
                bail!("old_agent_enc:{}:{}", server.id, server.version);
            }
            // Sealing QUIC came with 1.7: an older agent refuses a spec that has it.
            if obfs && !version_at_least(&server.version, (1, 7, 0)) {
                bail!("old_agent_obfs:{}:{}", server.id, server.version);
            }
            if self.needs_new_servers() && !version_at_least(&server.version, (1, 5, 0)) {
                bail!("old_agent:{}:{}", server.id, server.version);
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if !valid_name(&self.name) {
            bail!("bad_name");
        }
        if self.entry == self.exit {
            bail!("same_server");
        }
        if !matches!(self.mode.as_str(), "reverse" | "direct") {
            bail!("bad_mode");
        }
        if let Some(e) = &self.encryption {
            if !matches!(
                e.as_str(),
                "auto" | "aes-256-gcm" | "chacha20-poly1305" | "none"
            ) {
                bail!("bad_encryption");
            }
            // QUIC is always TLS 1.3: a cipher of its own does not exist there.
            if self.transport == "quic" && e != "auto" {
                bail!("encryption_quic");
            }
        }
        Ok(())
    }
}

/// Whether `version` (`1.5.0`, maybe with a suffix after a dash) is at least `min`. One that
/// cannot be read counts as too old.
fn version_at_least(version: &str, min: (u32, u32, u32)) -> bool {
    let mut parts = version
        .trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse::<u32>());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) => (a, b, c) >= min,
        _ => false,
    }
}

// ---- operations ----

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// A stable key for the browser to translate: `check_entry`, `write_exit`, ...
    pub id: String,
    /// `run`, `ok` or `fail`.
    pub state: &'static str,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Op {
    pub id: String,
    /// `create`, `edit`, `control` or `delete`.
    pub kind: &'static str,
    pub name: String,
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    pub steps: Vec<Step>,
    pub error: Option<String>,
    /// After a failure: whether the earlier steps were undone.
    pub undone: Option<bool>,
    /// Servers that could not be reached, and so were left out: a delete is kept for when
    /// they connect again, a start, stop or restart did not touch them.
    pub offline: Vec<String>,
}

/// The recent operations, for the browser to follow.
#[derive(Default)]
pub struct Ops(Mutex<Vec<Op>>);

impl Ops {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Op>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn begin(&self, kind: &'static str, name: &str) -> Result<String> {
        let id = random_hex(8)?;
        let mut ops = self.lock();
        // One operation per tunnel at a time.
        if ops.iter().any(|o| o.name == name && o.state == "running") {
            bail!("busy");
        }
        if ops.len() >= 30 {
            ops.remove(0);
        }
        ops.push(Op {
            id: id.clone(),
            kind,
            name: name.to_owned(),
            state: "running",
            steps: Vec::new(),
            error: None,
            undone: None,
            offline: Vec::new(),
        });
        Ok(id)
    }

    fn with(&self, id: &str, f: impl FnOnce(&mut Op)) {
        if let Some(op) = self.lock().iter_mut().find(|o| o.id == id) {
            f(op);
        }
    }

    /// A step starts.
    pub(crate) fn run(&self, id: &str, step: &str) {
        self.with(id, |op| {
            op.steps.push(Step {
                id: step.to_owned(),
                state: "run",
                detail: None,
            })
        });
    }

    /// A server could not be reached and was left out.
    pub(crate) fn left_out(&self, id: &str, server: &str) {
        self.with(id, |op| op.offline.push(server.to_owned()));
    }

    /// The last step ends.
    pub(crate) fn end(&self, id: &str, ok: bool, detail: Option<String>) {
        self.with(id, |op| {
            if let Some(step) = op.steps.last_mut() {
                step.state = if ok { "ok" } else { "fail" };
                step.detail = detail;
            }
        });
    }

    pub(crate) fn finish(&self, id: &str, error: Option<String>, undone: Option<bool>) {
        self.with(id, |op| {
            op.state = if error.is_some() { "failed" } else { "done" };
            op.error = error;
            op.undone = undone;
        });
    }

    /// Whether an operation of this kind is running.
    pub fn running(&self, kind: &str) -> bool {
        self.lock()
            .iter()
            .any(|o| o.kind == kind && o.state == "running")
    }

    pub fn get(&self, id: &str) -> Option<Op> {
        self.lock().iter().find(|o| o.id == id).cloned()
    }
}

/// Where a tunnel's two sides are, from what the servers last reported.
struct Placement {
    /// `(server id, its tunnel)` for each server that has a tunnel of that name.
    sides: Vec<(String, TunnelInfo)>,
}

fn place(hub: &Hub, name: &str) -> Result<Placement> {
    let mut sides = Vec::new();
    for server in hub.snapshot()? {
        if let Some(t) = server.tunnels.into_iter().find(|t| t.info.name == name) {
            sides.push((server.id, t.info));
        }
    }
    Ok(Placement { sides })
}

fn fail_text(reply: &Ack) -> String {
    reply.error.clone().unwrap_or_else(|| "failed".into())
}

/// Checks a request on both servers (nothing is written).
pub async fn check(hub: &Hub, req: &PairRequest) -> Result<(CheckReply, CheckReply)> {
    req.validate()?;
    req.require_new_enough(hub)?;
    let (entry, exit) = req.specs(None, None);
    let a = hub
        .ask_as(&req.entry, &Request::TunnelCheck { spec: entry })
        .await?;
    let b = hub
        .ask_as(&req.exit, &Request::TunnelCheck { spec: exit })
        .await?;
    Ok((a, b))
}

/// Waits until both sides say they are connected; the reason if they do not in time.
async fn wait_connected(hub: &Hub, req: &PairRequest, limit: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut last = String::new();
    loop {
        let mut connected = 0;
        for server in [&req.entry, &req.exit] {
            let tunnels: Vec<TunnelInfo> = match hub.ask_as(server, &Request::Tunnels).await {
                Ok(t) => t,
                Err(e) => {
                    last = format!("{e:#}");
                    continue;
                }
            };
            let Some(t) = tunnels.into_iter().find(|t| t.name == req.name) else {
                last = "the tunnel is not there".into();
                continue;
            };
            if let Some(e) = &t.error {
                last = e.clone();
            } else if t.active == Some(false) {
                last = "the service stopped".into();
            } else if let Some(status) = &t.status {
                if status.peer.connected {
                    connected += 1;
                } else if let Some(err) = &status.peer.last_error {
                    last = err.text.clone();
                }
            }
        }
        if connected == 2 {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(if last.is_empty() {
                "the two sides did not connect in time".into()
            } else {
                last
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn ctl(hub: &Hub, server: &str, name: &str, action: &str) -> Result<(), String> {
    let r: Ack = hub
        .ask_as(
            server,
            &Request::TunnelCtl {
                name: name.to_owned(),
                action: action.to_owned(),
            },
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
    if r.ok {
        Ok(())
    } else {
        Err(fail_text(&r))
    }
}

async fn put(hub: &Hub, server: &str, spec: Spec) -> Result<Option<String>, String> {
    let r: PutReply = hub
        .ask_as(server, &Request::TunnelPut { spec })
        .await
        .map_err(|e| format!("{e:#}"))?;
    if r.ok {
        Ok(r.pin)
    } else {
        Err(r.error.unwrap_or_else(|| "failed".into()))
    }
}

async fn remove(hub: &Hub, server: &str, name: &str) -> Result<(), String> {
    let r: Ack = hub
        .ask_as(
            server,
            &Request::TunnelDelete {
                name: name.to_owned(),
            },
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
    if r.ok {
        Ok(())
    } else {
        Err(fail_text(&r))
    }
}

fn describe(check: &CheckReply) -> String {
    let mut text = check.error.clone().unwrap_or_default();
    for c in &check.conflicts {
        text.push_str(&format!(
            "; {} {} is used by {}",
            c.proto,
            c.port,
            c.process.as_deref().unwrap_or("another program")
        ));
    }
    text
}

/// Starts making a tunnel. The operation's id comes back at once.
pub fn create(hub: &Arc<Hub>, req: PairRequest) -> Result<String> {
    req.validate()?;
    req.require_new_enough(hub)?;
    let id = hub.ops.begin("create", &req.name)?;
    let hub = hub.clone();
    let op = id.clone();
    tokio::spawn(async move {
        // A link made for this tunnel goes again if the tunnel cannot be made.
        let mut made_link = None;
        let outcome = run_create(&hub, &op, &req, &mut made_link).await;
        match outcome {
            Ok(()) => hub.ops.finish(&op, None, None),
            Err((error, undone)) => {
                if let Some(link) = made_link {
                    crate::netops::tear_down(&hub, &link).await;
                }
                hub.ops.finish(&op, Some(error), Some(undone));
            }
        }
    });
    Ok(id)
}

/// A request that asks for a private network, with its addresses filled in: the link between
/// the two servers is found (or made now, and put in `made_link`), and the tunnel listens on
/// and dials the address of its end on the server that accepts the connections (the exit in
/// direct mode, the entry in reverse mode). `None` when the request names no network.
pub async fn over_network(
    hub: &Hub,
    op: &str,
    req: &PairRequest,
    made_link: &mut Option<crate::networks::Link>,
) -> Result<Option<PairRequest>, String> {
    let Some(network) = &req.network else {
        return Ok(None);
    };
    let (link, fresh) = crate::netops::ensure_link(hub, op, network, &req.entry, &req.exit).await?;
    if fresh {
        *made_link = Some(link.clone());
    }
    let [acceptor, _] = req.acceptor_first();
    let endpoint = crate::netops::tunnel_endpoint(&link, acceptor, &req.listen);
    Ok(Some(PairRequest {
        listen: endpoint.clone(),
        dial: endpoint,
        network: None,
        ..req.clone()
    }))
}

async fn run_create(
    hub: &Hub,
    op: &str,
    req: &PairRequest,
    made_link: &mut Option<crate::networks::Link>,
) -> Result<(), (String, bool)> {
    let ops = &hub.ops;
    let step_fail = |ops: &Ops, e: String| {
        ops.end(op, false, Some(e.clone()));
        e
    };

    // Nothing of that name exists yet, on either server.
    ops.run(op, "look");
    if hub.delete_pending_for(&req.name) {
        return Err((step_fail(ops, "name_pending".into()), true));
    }
    match place(hub, &req.name) {
        Ok(p) if p.sides.is_empty() => ops.end(op, true, None),
        Ok(_) => return Err((step_fail(ops, "name_taken".into()), true)),
        Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
    }

    // Over a private network: the link between the two servers (made now if it is not
    // there), and the tunnel listens on and dials its addresses.
    let resolved = match over_network(hub, op, req, made_link).await {
        Ok(r) => r,
        Err(e) => return Err((e, true)),
    };
    let req = resolved.as_ref().unwrap_or(req);

    // Both servers agree it is valid, and its ports are free.
    for (step, server, spec) in [
        ("check_entry", &req.entry, req.specs(None, None).0),
        ("check_exit", &req.exit, req.specs(None, None).1),
    ] {
        ops.run(op, step);
        match hub
            .ask_as::<CheckReply>(server, &Request::TunnelCheck { spec })
            .await
        {
            Ok(c) if c.ok => ops.end(op, true, None),
            Ok(c) => return Err((step_fail(ops, describe(&c)), true)),
            Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
        }
    }

    let token = match random_hex(24) {
        Ok(t) => t,
        Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
    };
    let [first, second] = req.acceptor_first();
    let (first_role, second_role) = if req.mode == "reverse" {
        ("entry", "exit")
    } else {
        ("exit", "entry")
    };
    let mut written: Vec<&str> = Vec::new();
    let pin;

    // The accepting side first: the dialing side needs its address (and its pin) to be up.
    let (entry_spec, exit_spec) = req.specs(Some(&token), None);
    let first_spec = if first_role == "entry" {
        entry_spec
    } else {
        exit_spec
    };
    ops.run(op, &format!("write_{first_role}"));
    match put(hub, first, first_spec).await {
        Ok(p) => {
            pin = p;
            written.push(first);
            ops.end(op, true, None);
        }
        Err(e) => return Err((step_fail(ops, e), true)),
    }
    let (entry_spec, exit_spec) = req.specs(Some(&token), pin.as_deref());
    let second_spec = if second_role == "entry" {
        entry_spec
    } else {
        exit_spec
    };
    ops.run(op, &format!("write_{second_role}"));
    match put(hub, second, second_spec).await {
        Ok(_) => {
            written.push(second);
            ops.end(op, true, None);
        }
        Err(e) => {
            let undone = undo(hub, &written, &req.name).await;
            return Err((step_fail(ops, e), undone));
        }
    }

    for (role, server) in [(first_role, first), (second_role, second)] {
        ops.run(op, &format!("start_{role}"));
        match ctl(hub, server, &req.name, "enable").await {
            Ok(()) => ops.end(op, true, None),
            Err(e) => {
                let undone = undo(hub, &written, &req.name).await;
                return Err((step_fail(ops, e), undone));
            }
        }
    }

    ops.run(op, "connect");
    match wait_connected(hub, req, hub.connect_wait).await {
        Ok(()) => {
            ops.end(op, true, None);
            Ok(())
        }
        Err(e) => {
            let undone = undo(hub, &written, &req.name).await;
            Err((step_fail(ops, e), undone))
        }
    }
}

/// Removes what a failed operation made; whether all of it went.
async fn undo(hub: &Hub, servers: &[&str], name: &str) -> bool {
    let mut all = true;
    for server in servers.iter().rev() {
        all &= remove(hub, server, name).await.is_ok();
    }
    all
}

/// Starts editing a tunnel: both sides get the new settings, and if they do not connect
/// again the old ones are put back.
pub fn edit(hub: &Arc<Hub>, req: PairRequest) -> Result<String> {
    req.validate()?;
    req.require_new_enough(hub)?;
    let id = hub.ops.begin("edit", &req.name)?;
    let hub = hub.clone();
    let op = id.clone();
    tokio::spawn(async move {
        // A link made for this edit goes again if the edit cannot be made.
        let mut made_link = None;
        match run_edit(&hub, &op, &req, &mut made_link).await {
            Ok(()) => hub.ops.finish(&op, None, None),
            Err((error, undone)) => {
                if let Some(link) = made_link {
                    crate::netops::tear_down(&hub, &link).await;
                }
                hub.ops.finish(&op, Some(error), Some(undone));
            }
        }
    });
    Ok(id)
}

async fn run_edit(
    hub: &Hub,
    op: &str,
    req: &PairRequest,
    made_link: &mut Option<crate::networks::Link>,
) -> Result<(), (String, bool)> {
    let ops = &hub.ops;
    let step_fail = |ops: &Ops, e: String| {
        ops.end(op, false, Some(e.clone()));
        e
    };
    // What is there now, to put back.
    ops.run(op, "look");
    let mut before = Vec::new();
    for server in [&req.entry, &req.exit] {
        match hub
            .ask_as::<Spec>(
                server,
                &Request::TunnelGet {
                    name: req.name.clone(),
                },
            )
            .await
        {
            Ok(spec) => before.push((server.clone(), spec)),
            Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
        }
    }
    ops.end(op, true, None);

    // Over a private network (or back off it, when the request names none): the addresses
    // the tunnel listens on and dials.
    let resolved = match over_network(hub, op, req, made_link).await {
        Ok(r) => r,
        Err(e) => return Err((e, true)),
    };
    let req = resolved.as_ref().unwrap_or(req);

    let token = if req.rotate {
        match random_hex(24) {
            Ok(t) => Some(t),
            Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
        }
    } else {
        None
    };
    for (step, server, spec) in [
        ("check_entry", &req.entry, req.specs(None, None).0),
        ("check_exit", &req.exit, req.specs(None, None).1),
    ] {
        ops.run(op, step);
        match hub
            .ask_as::<CheckReply>(server, &Request::TunnelCheck { spec })
            .await
        {
            Ok(c) if c.ok => ops.end(op, true, None),
            Ok(c) => return Err((step_fail(ops, describe(&c)), true)),
            Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
        }
    }

    let [first, second] = req.acceptor_first();
    let mut pin = None;
    let mut changed: Vec<&str> = Vec::new();
    for (i, server) in [first, second].into_iter().enumerate() {
        let (entry_spec, exit_spec) = req.specs(token.as_deref(), pin.as_deref());
        let spec = if server == req.entry {
            entry_spec
        } else {
            exit_spec
        };
        let role = if server == req.entry { "entry" } else { "exit" };
        ops.run(op, &format!("write_{role}"));
        match put(hub, server, spec).await {
            Ok(p) => {
                if i == 0 {
                    pin = p;
                }
                changed.push(server);
                ops.end(op, true, None);
            }
            Err(e) => {
                let undone = restore(hub, &before, &changed, token.is_some()).await;
                return Err((step_fail(ops, e), undone));
            }
        }
    }
    for server in [first, second] {
        let role = if server == req.entry { "entry" } else { "exit" };
        ops.run(op, &format!("start_{role}"));
        match ctl(hub, server, &req.name, "restart").await {
            Ok(()) => ops.end(op, true, None),
            Err(e) => {
                let undone = restore(hub, &before, &changed, token.is_some()).await;
                return Err((step_fail(ops, e), undone));
            }
        }
    }
    ops.run(op, "connect");
    match wait_connected(hub, req, hub.connect_wait).await {
        Ok(()) => {
            ops.end(op, true, None);
            Ok(())
        }
        Err(e) => {
            let undone = restore(hub, &before, &changed, token.is_some()).await;
            Err((step_fail(ops, e), undone))
        }
    }
}

/// Puts the earlier settings back and restarts. A new token cannot be put back (the panel
/// never keeps tokens), so a failed rotation is reported as not undone.
async fn restore(hub: &Hub, before: &[(String, Spec)], changed: &[&str], rotated: bool) -> bool {
    let mut all = !rotated;
    for (server, spec) in before {
        if changed.contains(&server.as_str()) {
            all &= put(hub, server, spec.clone()).await.is_ok();
            all &= ctl(hub, server, &spec.name, "restart").await.is_ok();
        }
    }
    all
}

/// Starts, stops or restarts both sides, in the order that never leaves a dialer without
/// its acceptor.
pub fn control(hub: &Arc<Hub>, name: &str, action: &str) -> Result<String> {
    if !valid_name(name) {
        bail!("bad_name");
    }
    if !matches!(action, "start" | "stop" | "restart") {
        bail!("bad_action");
    }
    let id = hub.ops.begin("control", name)?;
    let hub = hub.clone();
    let (op, name, action) = (id.clone(), name.to_owned(), action.to_owned());
    tokio::spawn(async move {
        let error = run_control(&hub, &op, &name, &action).await.err();
        hub.ops.finish(&op, error, None);
    });
    Ok(id)
}

/// The tunnel's servers, the accepting side first.
fn ordered(p: &Placement) -> Vec<(String, TunnelInfo)> {
    let mut sides = p.sides.clone();
    sides.sort_by_key(|(_, t)| {
        let accepts = matches!(
            (t.role.as_str(), t.mode.as_str()),
            ("entry", "reverse") | ("exit", "direct")
        );
        !accepts
    });
    sides
}

async fn run_control(hub: &Hub, op: &str, name: &str, action: &str) -> Result<(), String> {
    let ops = &hub.ops;
    ops.run(op, "look");
    let mut sides = match place(hub, name) {
        Ok(p) if !p.sides.is_empty() => ordered(&p),
        Ok(_) => {
            ops.end(op, false, Some("no_such_tunnel".into()));
            return Err("no_such_tunnel".into());
        }
        Err(e) => {
            ops.end(op, false, Some(format!("{e:#}")));
            return Err(format!("{e:#}"));
        }
    };
    ops.end(op, true, None);
    if action == "stop" {
        sides.reverse();
    }
    // A tunnel can be stopped (or started) from the side that is reachable.
    if sides.iter().all(|(s, _)| !hub.is_online(s)) {
        ops.run(op, "look");
        ops.end(op, false, Some("that server is not connected".into()));
        return Err("that server is not connected".into());
    }
    for (server, info) in sides {
        ops.run(op, &format!("{action}_{}", info.role));
        if !hub.is_online(&server) {
            ops.end(op, true, Some("offline".into()));
            ops.left_out(op, &server);
            continue;
        }
        match ctl(hub, &server, name, action).await {
            Ok(()) => ops.end(op, true, None),
            Err(e) => {
                ops.end(op, false, Some(e.clone()));
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Stops and removes both sides.
pub fn delete(hub: &Arc<Hub>, name: &str) -> Result<String> {
    if !valid_name(name) {
        bail!("bad_name");
    }
    let id = hub.ops.begin("delete", name)?;
    let hub = hub.clone();
    let (op, name) = (id.clone(), name.to_owned());
    tokio::spawn(async move {
        let error = run_delete(&hub, &op, &name).await.err();
        hub.ops.finish(&op, error, None);
    });
    Ok(id)
}

async fn run_delete(hub: &Hub, op: &str, name: &str) -> Result<(), String> {
    let ops = &hub.ops;
    ops.run(op, "look");
    let mut sides = match place(hub, name) {
        Ok(p) if !p.sides.is_empty() => ordered(&p),
        Ok(_) => {
            ops.end(op, false, Some("no_such_tunnel".into()));
            return Err("no_such_tunnel".into());
        }
        Err(e) => {
            ops.end(op, false, Some(format!("{e:#}")));
            return Err(format!("{e:#}"));
        }
    };
    ops.end(op, true, None);
    // The dialing side goes first, so it does not retry against a closed acceptor.
    sides.reverse();
    let mut failed = None;
    for (server, info) in sides {
        ops.run(op, &format!("delete_{}", info.role));
        // A server that is away cannot be asked. The delete is kept for it (its agent does
        // it when it connects again) and the tunnel is gone from the lists now.
        if !hub.is_online(&server) {
            match hub.queue_delete(&server, name) {
                Ok(()) => {
                    ops.end(op, true, Some("queued".into()));
                    ops.left_out(op, &server);
                }
                Err(e) => {
                    ops.end(op, false, Some(format!("{e:#}")));
                    failed = Some(format!("{e:#}"));
                }
            }
            continue;
        }
        match remove(hub, &server, name).await {
            Ok(()) => ops.end(op, true, None),
            Err(e) => {
                ops.end(op, false, Some(e.clone()));
                failed = Some(e);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// One line of a tunnel's log, from one of its sides.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub server: String,
    pub role: String,
    pub text: String,
}

/// The last `lines` lines of a tunnel's log from both sides, in time order (the journal's
/// lines start with an ISO time, which sorts as text).
pub async fn logs(hub: &Hub, name: &str, lines: u32) -> Result<Vec<LogLine>> {
    if !valid_name(name) {
        bail!("bad_name");
    }
    let placement = place(hub, name)?;
    if placement.sides.is_empty() {
        bail!("no_such_tunnel");
    }
    let mut all: Vec<LogLine> = Vec::new();
    for (server, info) in placement.sides {
        let reply: TextReply = match hub
            .ask_as(
                &server,
                &Request::Logs {
                    name: name.to_owned(),
                    lines,
                },
            )
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        all.extend(
            reply
                .text
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with("-- "))
                .map(|l| LogLine {
                    server: server.clone(),
                    role: info.role.clone(),
                    text: l.to_owned(),
                }),
        );
    }
    all.sort_by(|a, b| a.text.cmp(&b.text));
    let skip = all.len().saturating_sub(lines as usize);
    Ok(all.split_off(skip))
}

/// Runs the speed test of a tunnel on its entry side and returns what it printed.
/// Gets a Let's Encrypt certificate for `host` on `server` (for a wss tunnel that listens
/// there). It can take a minute: certbot is installed first if the server has none.
pub async fn certificate(
    hub: &Hub,
    server: &str,
    host: &str,
    email: Option<&str>,
) -> Result<crate::wire::CertReply> {
    let raw = hub
        .ask_within(
            server,
            &Request::TunnelCert {
                domain: host.to_owned(),
                email: email.map(str::to_owned),
            },
            Duration::from_secs(660),
        )
        .await?;
    serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("the server's answer was not understood (an older agent cannot get certificates: update it)"))
}

/// The server that runs a tunnel's speed test: the one with its entry side.
fn speedtest_server(hub: &Hub, name: &str) -> Result<String> {
    if !valid_name(name) {
        bail!("bad_name");
    }
    let placement = place(hub, name)?;
    placement
        .sides
        .into_iter()
        .find(|(_, t)| t.role == "entry")
        .map(|(server, _)| server)
        .ok_or_else(|| anyhow::anyhow!("no_such_tunnel"))
}

/// Starts a speed test that can be followed while it runs; its id comes back.
pub async fn speedtest_start(
    hub: &Hub,
    name: &str,
    seconds: u32,
    streams: u32,
    udp: bool,
) -> Result<crate::wire::SpeedStarted> {
    let server = speedtest_server(hub, name)?;
    let raw = hub
        .ask(
            &server,
            &Request::SpeedtestStart {
                name: name.to_owned(),
                seconds,
                streams,
                udp,
            },
        )
        .await?;
    serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("the server's answer was not understood"))
}

/// Stops a started speed test.
pub async fn speedtest_stop(hub: &Hub, name: &str, id: &str) -> Result<()> {
    let server = speedtest_server(hub, name)?;
    let raw = hub
        .ask(&server, &Request::SpeedtestStop { id: id.to_owned() })
        .await?;
    let ack: Ack = serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("the server's answer was not understood"))?;
    if ack.ok {
        Ok(())
    } else {
        bail!(ack.error.unwrap_or_else(|| "failed".into()))
    }
}

/// What a started speed test has said since `after` lines.
pub async fn speedtest_poll(
    hub: &Hub,
    name: &str,
    id: &str,
    after: u32,
) -> Result<crate::wire::SpeedPoll> {
    let server = speedtest_server(hub, name)?;
    let raw = hub
        .ask(
            &server,
            &Request::SpeedtestPoll {
                id: id.to_owned(),
                after,
            },
        )
        .await?;
    serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("the server's answer was not understood"))
}

pub async fn speedtest(
    hub: &Hub,
    name: &str,
    seconds: u32,
    streams: u32,
    udp: bool,
) -> Result<SpeedReply> {
    if !valid_name(name) {
        bail!("bad_name");
    }
    let placement = place(hub, name)?;
    let Some((server, _)) = placement.sides.iter().find(|(_, t)| t.role == "entry") else {
        bail!("no_such_tunnel");
    };
    let seconds = seconds.clamp(1, 60);
    let limit = Duration::from_secs(u64::from(seconds) * 3 + 50);
    let raw = hub
        .ask_within(
            server,
            &Request::Speedtest {
                name: name.to_owned(),
                seconds,
                streams,
                udp,
            },
            limit,
        )
        .await?;
    serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("the server's answer was not understood"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_compared_by_their_numbers() {
        let min = (1, 5, 0);
        assert!(version_at_least("1.5.0", min) && version_at_least("v1.10.2", min));
        assert!(version_at_least("2.0.0-rc1", min) && version_at_least("1.5.0-dev", min));
        assert!(!version_at_least("1.4.0", min) && !version_at_least("1.4.99", min));
        assert!(!version_at_least("", min) && !version_at_least("unknown", min));
    }

    #[test]
    fn a_cipher_must_be_one_the_core_knows_and_not_on_quic() {
        let req = |transport: &str, enc: Option<&str>| PairRequest {
            name: "t".into(),
            entry: "a".into(),
            exit: "b".into(),
            mode: "reverse".into(),
            transport: transport.into(),
            profile: None,
            listen: "0.0.0.0:3080".into(),
            dial: "1.2.3.4:3080".into(),
            pool: None,
            ws_path: None,
            ws_host: None,
            tls_sni: None,
            mux: None,
            tls_cert: None,
            tls_key: None,
            encryption: enc.map(str::to_owned),
            quic_obfs: false,
            forwards: Vec::new(),
            rotate: false,
            network: None,
        };
        for ok in ["auto", "aes-256-gcm", "chacha20-poly1305", "none"] {
            assert!(req("tcpmux", Some(ok)).validate().is_ok(), "{ok}");
        }
        assert!(req("tcpmux", None).validate().is_ok());
        assert_eq!(
            req("tcpmux", Some("rot13"))
                .validate()
                .unwrap_err()
                .to_string(),
            "bad_encryption"
        );
        // QUIC has no cipher of its own to choose, and auto is always fine.
        assert_eq!(
            req("quic", Some("none"))
                .validate()
                .unwrap_err()
                .to_string(),
            "encryption_quic"
        );
        assert!(req("quic", Some("auto")).validate().is_ok());
        // `auto` is not written: the side stays on the default.
        assert_eq!(
            req("tcpmux", Some("auto")).specs(None, None).0.encryption,
            None
        );
        assert_eq!(
            req("tcpmux", Some("none"))
                .specs(None, None)
                .1
                .encryption
                .as_deref(),
            Some("none")
        );
    }

    #[test]
    fn sealed_quic_is_for_quic_only_and_needs_a_new_agent() {
        let req = |transport: &str, obfs: bool| PairRequest {
            name: "t".into(),
            entry: "a".into(),
            exit: "b".into(),
            mode: "reverse".into(),
            transport: transport.into(),
            profile: None,
            listen: "0.0.0.0:3080".into(),
            dial: "1.2.3.4:3080".into(),
            pool: None,
            ws_path: None,
            ws_host: None,
            tls_sni: None,
            mux: None,
            encryption: None,
            quic_obfs: obfs,
            tls_cert: None,
            tls_key: None,
            forwards: Vec::new(),
            rotate: false,
            network: None,
        };
        // Both sides get it over quic, and nobody over another transport.
        let (entry, exit) = req("quic", true).specs(None, None);
        assert!(entry.quic_obfs && exit.quic_obfs);
        let (entry, exit) = req("tcpmux", true).specs(None, None);
        assert!(!entry.quic_obfs && !exit.quic_obfs);
        let (entry, _) = req("quic", false).specs(None, None);
        assert!(!entry.quic_obfs);
        assert!(req("quic", true).validate().is_ok());
        // Before 1.7 an agent does not know the field, and 1.7 and later do.
        assert!(!version_at_least("1.6.0", (1, 7, 0)));
        assert!(version_at_least("1.7.0", (1, 7, 0)));
        assert!(version_at_least("v1.8.2", (1, 7, 0)));
    }

    #[test]
    fn auto_and_mux_settings_need_new_servers() {
        let req = |transport: &str, mux: Option<crate::wire::MuxSpec>| PairRequest {
            name: "t".into(),
            entry: "a".into(),
            exit: "b".into(),
            mode: "reverse".into(),
            transport: transport.into(),
            profile: None,
            listen: "0.0.0.0:3080".into(),
            dial: "1.2.3.4:3080".into(),
            pool: None,
            ws_path: None,
            ws_host: None,
            tls_sni: None,
            mux,
            quic_obfs: false,
            tls_cert: None,
            tls_key: None,
            encryption: None,
            forwards: Vec::new(),
            rotate: false,
            network: None,
        };
        assert!(req("auto", None).needs_new_servers());
        let tuned = crate::wire::MuxSpec {
            connections: Some(4),
            ..Default::default()
        };
        assert!(req("tcpmux", Some(tuned)).needs_new_servers());
        assert!(!req("tcpmux", Some(Default::default())).needs_new_servers());
        assert!(!req("kcp", None).needs_new_servers());
    }
}
