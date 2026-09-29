//! A tunnel is a pair: the entry on one server and the exit on another. This makes, edits,
//! controls and deletes both sides as one operation (docs/PHASE12.md, section 3). Each
//! operation runs in the background as a list of steps that the browser follows, and a
//! step that fails undoes what the earlier ones did.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::random_hex;
use crate::hub::Hub;
use crate::manage::valid_name;
use crate::wire::{Ack, CheckReply, ForwardInfo, PutReply, Request, Spec, TextReply, TunnelInfo};

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
    #[serde(default)]
    pub forwards: Vec<ForwardInfo>,
    /// Edit only: make a new token for both sides.
    #[serde(default)]
    pub rotate: bool,
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
            tls_sni: (!accepts).then(|| self.tls_sni.clone()).flatten(),
            tls_pin: (!accepts && self.transport == "wss")
                .then(|| pin.map(str::to_owned))
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
        Ok(())
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
}

/// The recent operations, for the browser to follow.
#[derive(Default)]
pub struct Ops(Mutex<Vec<Op>>);

impl Ops {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Op>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn begin(&self, kind: &'static str, name: &str) -> Result<String> {
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
        });
        Ok(id)
    }

    fn with(&self, id: &str, f: impl FnOnce(&mut Op)) {
        if let Some(op) = self.lock().iter_mut().find(|o| o.id == id) {
            f(op);
        }
    }

    /// A step starts.
    fn run(&self, id: &str, step: &str) {
        self.with(id, |op| {
            op.steps.push(Step {
                id: step.to_owned(),
                state: "run",
                detail: None,
            })
        });
    }

    /// The last step ends.
    fn end(&self, id: &str, ok: bool, detail: Option<String>) {
        self.with(id, |op| {
            if let Some(step) = op.steps.last_mut() {
                step.state = if ok { "ok" } else { "fail" };
                step.detail = detail;
            }
        });
    }

    fn finish(&self, id: &str, error: Option<String>, undone: Option<bool>) {
        self.with(id, |op| {
            op.state = if error.is_some() { "failed" } else { "done" };
            op.error = error;
            op.undone = undone;
        });
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
    let id = hub.ops.begin("create", &req.name)?;
    let hub = hub.clone();
    let op = id.clone();
    tokio::spawn(async move {
        let outcome = run_create(&hub, &op, &req).await;
        match outcome {
            Ok(()) => hub.ops.finish(&op, None, None),
            Err((error, undone)) => hub.ops.finish(&op, Some(error), Some(undone)),
        }
    });
    Ok(id)
}

async fn run_create(hub: &Hub, op: &str, req: &PairRequest) -> Result<(), (String, bool)> {
    let ops = &hub.ops;
    let step_fail = |ops: &Ops, e: String| {
        ops.end(op, false, Some(e.clone()));
        e
    };

    // Nothing of that name exists yet, on either server.
    ops.run(op, "look");
    match place(hub, &req.name) {
        Ok(p) if p.sides.is_empty() => ops.end(op, true, None),
        Ok(_) => return Err((step_fail(ops, "name_taken".into()), true)),
        Err(e) => return Err((step_fail(ops, format!("{e:#}")), true)),
    }

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
    let id = hub.ops.begin("edit", &req.name)?;
    let hub = hub.clone();
    let op = id.clone();
    tokio::spawn(async move {
        match run_edit(&hub, &op, &req).await {
            Ok(()) => hub.ops.finish(&op, None, None),
            Err((error, undone)) => hub.ops.finish(&op, Some(error), Some(undone)),
        }
    });
    Ok(id)
}

async fn run_edit(hub: &Hub, op: &str, req: &PairRequest) -> Result<(), (String, bool)> {
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
    for (server, info) in sides {
        ops.run(op, &format!("{action}_{}", info.role));
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
pub async fn speedtest(
    hub: &Hub,
    name: &str,
    seconds: u32,
    streams: u32,
    udp: bool,
) -> Result<TextReply> {
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
