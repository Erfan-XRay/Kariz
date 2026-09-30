//! What an agent does to its server's tunnels when the panel asks: check and write a config from a structured spec, read it back, control its
//! service, delete it, list the listening ports, read the log, run the speed test.
//!
//! Nothing here takes a command, a path or a piece of config text from the panel: the file
//! is rendered from typed fields (so a value cannot add a line to it), the name is checked
//! against a strict pattern, and the programs that run get fixed arguments.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use kariz::config::{mode_name, role_name, Config, Role};

use crate::wire::{CheckReply, ForwardInfo, PortOwner, Spec, SpeedReply, TextReply};

const PLACEHOLDER_TOKEN: &str = "0000000000000000000000000000000000000000000000000000";

/// A tunnel's name is its file name and its service (`kariz@NAME`): small letters, digits
/// and dashes, and it starts with a letter or a digit.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn config_path(dir: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!("a tunnel name is 1 to 32 characters: small letters, digits and dashes");
    }
    Ok(dir.join(format!("{name}.toml")))
}

/// The token a config already holds.
fn existing_token(path: &Path) -> Option<String> {
    let value: toml::Value = toml::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(value.get("tunnel")?.get("token")?.as_str()?.to_owned())
}

/// The config file for a spec, with `token` in it.
pub fn render(spec: &Spec, token: &str, cert_files: Option<(&Path, &Path)>) -> Result<String> {
    use toml::{Table, Value};
    let text = |s: &str| Value::String(s.to_owned());
    let mut root = Table::new();
    root.insert("role".into(), text(&spec.role));
    root.insert("mode".into(), text(&spec.mode));
    if let Some(profile) = &spec.profile {
        root.insert("profile".into(), text(profile));
    }
    let mut tunnel = Table::new();
    tunnel.insert("transport".into(), text(&spec.transport));
    if let Some(listen) = &spec.listen {
        tunnel.insert("listen".into(), text(listen));
    }
    if let Some(remote) = &spec.remote {
        tunnel.insert("remote".into(), text(remote));
    }
    tunnel.insert("token".into(), text(token));
    if let Some(pool) = spec.pool {
        tunnel.insert("pool".into(), Value::Integer(i64::from(pool)));
    }
    if spec.ws_path.is_some() || spec.ws_host.is_some() {
        let mut ws = Table::new();
        if let Some(path) = &spec.ws_path {
            ws.insert("path".into(), text(path));
        }
        if let Some(host) = &spec.ws_host {
            ws.insert("host".into(), text(host));
        }
        tunnel.insert("ws".into(), Value::Table(ws));
    }
    if let Some((cert, key)) = cert_files {
        // A listening wss side serves the certificate this agent made for it.
        let mut tls = Table::new();
        tls.insert("cert".into(), text(&cert.to_string_lossy()));
        tls.insert("key".into(), text(&key.to_string_lossy()));
        tunnel.insert("tls".into(), Value::Table(tls));
    } else if spec.tls_sni.is_some() || spec.tls_pin.is_some() {
        let mut tls = Table::new();
        if let Some(sni) = &spec.tls_sni {
            tls.insert("sni".into(), text(sni));
        }
        if let Some(pin) = &spec.tls_pin {
            tls.insert("pin_sha256".into(), text(pin));
        }
        tunnel.insert("tls".into(), Value::Table(tls));
    }
    root.insert("tunnel".into(), Value::Table(tunnel));
    let forwards: Vec<Value> = spec
        .forwards
        .iter()
        .map(|f| {
            let mut t = Table::new();
            t.insert("listen".into(), text(&f.listen));
            t.insert("target".into(), text(&f.target));
            t.insert("protocol".into(), text(&f.protocol));
            Value::Table(t)
        })
        .collect();
    if !forwards.is_empty() {
        root.insert("forward".into(), Value::Array(forwards));
    }
    toml::to_string_pretty(&root).context("failed to render the config")
}

/// Renders and validates a spec (the token of an existing file is kept when the spec has
/// none). The text and the parsed config come back.
fn build(dir: &Path, spec: &Spec, for_check: bool) -> Result<(String, Config)> {
    let path = config_path(dir, &spec.name)?;
    let token = match (&spec.token, existing_token(&path)) {
        (Some(t), _) => t.clone(),
        (None, Some(t)) => t,
        (None, None) if for_check => PLACEHOLDER_TOKEN.to_owned(),
        (None, None) => bail!("a new tunnel needs a token"),
    };
    if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        bail!("the token is too short or has characters it should not");
    }
    plain_text(spec)?;
    let files = cert_files(dir, spec);
    let text = render(
        spec,
        &token,
        files.as_ref().map(|(c, k)| (c.as_path(), k.as_path())),
    )?;
    let config = Config::parse(&text)?;
    Ok((text, config))
}

/// Whether this side of the spec listens for the tunnel.
fn is_acceptor(spec: &Spec) -> bool {
    matches!(
        (spec.role.as_str(), spec.mode.as_str()),
        ("entry", "reverse") | ("exit", "direct")
    )
}

/// The certificate and key of a listening wss side, beside its config.
fn cert_files(dir: &Path, spec: &Spec) -> Option<(PathBuf, PathBuf)> {
    (spec.transport == "wss" && is_acceptor(spec)).then(|| {
        (
            dir.join(format!("{}.crt", spec.name)),
            dir.join(format!("{}.key", spec.name)),
        )
    })
}

/// Every text field of a spec is one plain line: no control characters, so nothing a
/// panel sends can break out of its value.
fn plain_text(spec: &Spec) -> Result<()> {
    let optional = [
        &spec.profile,
        &spec.listen,
        &spec.remote,
        &spec.ws_path,
        &spec.ws_host,
        &spec.tls_sni,
        &spec.tls_pin,
    ];
    let fields = [&spec.role, &spec.mode, &spec.transport]
        .into_iter()
        .chain(optional.into_iter().flatten())
        .chain(
            spec.forwards
                .iter()
                .flat_map(|f| [&f.listen, &f.target, &f.protocol]),
        );
    for value in fields {
        if value.len() > 512 || value.chars().any(char::is_control) {
            bail!("a value is too long or has control characters in it");
        }
    }
    Ok(())
}

/// The (protocol, port) pairs a spec listens on.
pub fn wanted_ports(spec: &Spec) -> Vec<(&'static str, u16)> {
    let mut out = Vec::new();
    let port = |addr: &str| addr.rsplit_once(':').map_or(addr, |(_, p)| p).parse().ok();
    if is_acceptor(spec) {
        let udp = matches!(spec.transport.as_str(), "quic" | "kcp");
        if let Some(p) = spec.listen.as_deref().and_then(port) {
            out.push((if udp { "udp" } else { "tcp" }, p));
        }
    }
    if spec.role == "entry" {
        for f in &spec.forwards {
            let Some(p) = port(&f.listen) else { continue };
            if f.protocol.contains("tcp") || f.protocol.is_empty() {
                out.push(("tcp", p));
            }
            if f.protocol.contains("udp") {
                out.push(("udp", p));
            }
        }
    }
    out
}

/// Validates a spec on this server and says which of its ports are taken by others.
pub fn check(dir: &Path, owners: &[PortOwner], spec: &Spec) -> CheckReply {
    let config = match build(dir, spec, true) {
        Ok((_, c)) => c,
        Err(e) => {
            return CheckReply {
                ok: false,
                error: Some(format!("{e:#}")),
                ..Default::default()
            }
        }
    };
    // What the tunnel of this name holds now is not a conflict: it is being replaced.
    let own: Vec<(&str, u16)> = get(dir, &spec.name)
        .map(|old| wanted_ports(&old))
        .unwrap_or_default();
    let conflicts: Vec<PortOwner> = wanted_ports(spec)
        .into_iter()
        .filter(|w| !own.contains(w))
        .flat_map(|(proto, port)| {
            owners
                .iter()
                .filter(move |o| o.proto == proto && o.port == port)
                .cloned()
        })
        .collect();
    CheckReply {
        ok: conflicts.is_empty(),
        error: (!conflicts.is_empty()).then(|| "a port is already in use".to_owned()),
        warnings: config.warnings().iter().map(|w| (*w).to_owned()).collect(),
        conflicts,
    }
}

/// Writes the config (0600), keeping the previous file as `NAME.toml.bak`. A listening wss
/// side gets a self-signed certificate made for it (once); its pin comes back, for the
/// dialing side.
pub fn put(dir: &Path, spec: &Spec) -> Result<Option<String>> {
    let (text, _) = build(dir, spec, false)?;
    let path = config_path(dir, &spec.name)?;
    let pin = match cert_files(dir, spec) {
        Some((cert, key)) => Some(crate::cert::ensure(&cert, &key)?),
        None => None,
    };
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        let old = std::fs::read(&path)?;
        crate::agent::write_private(Path::new(&bak), &old)?;
    }
    crate::agent::write_private(&path, text.as_bytes())?;
    Ok(pin)
}

/// The spec of an existing tunnel, without its token.
pub fn get(dir: &Path, name: &str) -> Result<Spec> {
    let path = config_path(dir, name)?;
    let c = Config::load(&path)?;
    Ok(Spec {
        name: name.to_owned(),
        role: role_name(c.role).to_owned(),
        mode: mode_name(c.mode).to_owned(),
        transport: c.tunnel.transport.name().to_owned(),
        profile: Some(c.profile.name().to_owned()),
        listen: c.tunnel.listen.clone(),
        remote: c.tunnel.remote.clone(),
        token: None,
        pool: Some(u32::try_from(c.tunnel.pool).unwrap_or(8)),
        ws_path: c.tunnel.ws.as_ref().map(|w| w.path.clone()),
        ws_host: c.tunnel.ws.as_ref().and_then(|w| w.host.clone()),
        tls_sni: c.tunnel.tls.as_ref().and_then(|t| t.sni.clone()),
        tls_pin: c.tunnel.tls.as_ref().and_then(|t| {
            t.pin_sha256.clone().or_else(|| {
                t.cert
                    .as_deref()
                    .and_then(|p| crate::cert::fingerprint(p).ok())
            })
        }),
        forwards: c
            .forward
            .iter()
            .map(|f| ForwardInfo {
                listen: f.listen.clone(),
                target: f.target.clone(),
                protocol: f.protocol.name().to_owned(),
            })
            .collect(),
    })
}

/// Removes the config, its backup and its control socket (the service is stopped first by
/// the caller).
pub fn remove_files(dir: &Path, name: &str) -> Result<()> {
    let path = config_path(dir, name)?;
    for p in [
        path.clone(),
        path.with_extension("toml.bak"),
        path.with_extension("sock"),
        path.with_extension("crt"),
        path.with_extension("key"),
    ] {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("failed to remove {}", p.display())),
        }
    }
    Ok(())
}

// ---- listening ports ----

fn parse_addr(hex: &str) -> Option<String> {
    match hex.len() {
        8 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some(std::net::Ipv4Addr::from(n.swap_bytes()).to_string())
        }
        32 => {
            let mut bytes = [0u8; 16];
            for (i, chunk) in hex.as_bytes().chunks(8).enumerate() {
                let word = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
                bytes[i * 4..i * 4 + 4].copy_from_slice(&word.swap_bytes().to_be_bytes());
            }
            Some(std::net::Ipv6Addr::from(bytes).to_string())
        }
        _ => None,
    }
}

/// The listening sockets in a `/proc/net/{tcp,tcp6,udp,udp6}` file: (address, port, inode).
pub fn parse_proc_net(text: &str, udp: bool) -> Vec<(String, u16, u64)> {
    let listening = if udp { "07" } else { "0A" };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != listening {
                return None;
            }
            let (addr, port) = f[1].split_once(':')?;
            let port = u16::from_str_radix(port, 16).ok()?;
            if port == 0 {
                return None;
            }
            Some((parse_addr(addr)?, port, f[9].parse().ok()?))
        })
        .collect()
}

/// Which process holds each socket inode, from `/proc/*/fd`.
#[cfg(target_os = "linux")]
fn inode_owners(wanted: &[u64]) -> HashMap<u64, (u32, String)> {
    let mut found = HashMap::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in procs.filter_map(Result::ok) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.filter_map(Result::ok) {
            let Ok(link) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let link = link.to_string_lossy();
            let Some(inode) = link
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            if wanted.contains(&inode) && !found.contains_key(&inode) {
                let comm = std::fs::read_to_string(entry.path().join("comm"))
                    .map(|c| c.trim().to_owned())
                    .unwrap_or_default();
                found.insert(inode, (pid, comm));
            }
        }
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn inode_owners(_: &[u64]) -> HashMap<u64, (u32, String)> {
    HashMap::new()
}

/// Every TCP and UDP port this server listens on, with the process that owns it.
pub fn ports() -> Vec<PortOwner> {
    let mut sockets = Vec::new();
    for (file, proto) in [
        ("/proc/net/tcp", "tcp"),
        ("/proc/net/tcp6", "tcp"),
        ("/proc/net/udp", "udp"),
        ("/proc/net/udp6", "udp"),
    ] {
        if let Ok(text) = std::fs::read_to_string(file) {
            for (addr, port, inode) in parse_proc_net(&text, proto == "udp") {
                sockets.push((proto, addr, port, inode));
            }
        }
    }
    let owners = inode_owners(&sockets.iter().map(|s| s.3).collect::<Vec<_>>());
    let mut out: Vec<PortOwner> = sockets
        .into_iter()
        .map(|(proto, addr, port, inode)| {
            let owner = owners.get(&inode);
            PortOwner {
                proto: proto.to_owned(),
                addr,
                port,
                process: owner.map(|o| o.1.clone()).filter(|c| !c.is_empty()),
                pid: owner.map(|o| o.0),
            }
        })
        .collect();
    out.sort_by(|a, b| (a.port, &a.proto, &a.addr).cmp(&(b.port, &b.proto, &b.addr)));
    out.dedup();
    out
}

// ---- systemd, the journal, the speed test ----

/// The `systemctl` arguments for an action on a tunnel's service.
pub fn ctl_args(name: &str, action: &str) -> Result<Vec<String>> {
    if !valid_name(name) {
        bail!("a tunnel name is 1 to 32 characters: small letters, digits and dashes");
    }
    let unit = format!("kariz@{name}");
    Ok(match action {
        "start" | "stop" | "restart" => vec![action.to_owned(), unit],
        "enable" | "disable" => vec![action.to_owned(), "--now".to_owned(), unit],
        _ => bail!("unknown action {action}"),
    })
}

/// Runs a program with fixed arguments and a time limit; its output, or why it failed.
pub(crate) async fn run(program: &str, args: &[String], limit: Duration) -> Result<(bool, String)> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| anyhow!("{program} took longer than {} s", limit.as_secs()))?
        .with_context(|| format!("failed to run {program}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.truncate(512 * 1024);
    Ok((output.status.success(), text))
}

/// How a server runs its tunnels: `systemd` (the default) or `process`, as child
/// processes of the panel or the agent, for a host that has no systemd.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    #[default]
    Systemd,
    Process,
}

impl ServiceKind {
    pub fn services(self, dir: &Path) -> std::sync::Arc<dyn Services> {
        match self {
            Self::Systemd => std::sync::Arc::new(Systemd),
            Self::Process => std::sync::Arc::new(Processes::new(dir, &kariz_binary())),
        }
    }
}

pub type Fut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// How a server starts, stops and asks about its tunnels' services. The agent uses
/// systemd; a host without it (a container) can run the daemons as child processes, and
/// the tests do.
pub trait Services: Send + Sync {
    /// `start`, `stop`, `restart`, `enable` or `disable` (the last two also start and
    /// stop).
    fn ctl(&self, name: String, action: String) -> Fut<Result<()>>;
    /// Whether the tunnel runs; `None` when this cannot be known.
    fn active(&self, name: String) -> Fut<Option<bool>>;
}

/// The services of a Linux server with systemd: `kariz@NAME`.
pub struct Systemd;

impl Services for Systemd {
    fn ctl(&self, name: String, action: String) -> Fut<Result<()>> {
        Box::pin(async move {
            let args = ctl_args(&name, &action)?;
            let (ok, text) = run("systemctl", &args, Duration::from_secs(30)).await?;
            if ok {
                Ok(())
            } else {
                bail!("systemctl {action} failed: {}", text.trim())
            }
        })
    }

    #[cfg(target_os = "linux")]
    fn active(&self, name: String) -> Fut<Option<bool>> {
        Box::pin(async move {
            let args = [
                "is-active".to_owned(),
                "--quiet".to_owned(),
                format!("kariz@{name}"),
            ];
            match run("systemctl", &args, Duration::from_secs(3)).await {
                Ok((ok, _)) => Some(ok),
                Err(_) => None,
            }
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn active(&self, _: String) -> Fut<Option<bool>> {
        Box::pin(async { None })
    }
}

/// Daemons run as child processes of the agent, for hosts without systemd. They stop
/// with the agent.
pub struct Processes {
    dir: PathBuf,
    binary: PathBuf,
    children: std::sync::Mutex<HashMap<String, tokio::process::Child>>,
}

impl Processes {
    pub fn new(dir: &Path, binary: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            binary: binary.to_path_buf(),
            children: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn start(&self, name: &str) -> Result<()> {
        let mut children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = children.get_mut(name) {
            if matches!(child.try_wait(), Ok(None)) {
                return Ok(());
            }
        }
        let child = tokio::process::Command::new(&self.binary)
            .arg("run")
            .arg("-c")
            .arg(config_path(&self.dir, name)?)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", self.binary.display()))?;
        children.insert(name.to_owned(), child);
        Ok(())
    }

    fn stop(&self, name: &str) {
        let child = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        if let Some(mut child) = child {
            let _ = child.start_kill();
        }
    }
}

impl Services for Processes {
    fn ctl(&self, name: String, action: String) -> Fut<Result<()>> {
        let result = ctl_args(&name, &action).and_then(|_| match action.as_str() {
            "start" | "enable" => self.start(&name),
            "stop" | "disable" => {
                self.stop(&name);
                Ok(())
            }
            _ => {
                self.stop(&name);
                self.start(&name)
            }
        });
        Box::pin(async move { result })
    }

    fn active(&self, name: String) -> Fut<Option<bool>> {
        let running = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&name)
            .map(|c| matches!(c.try_wait(), Ok(None)));
        Box::pin(async move { Some(running.unwrap_or(false)) })
    }
}

/// Stops the service, then removes the files.
pub async fn delete(dir: &Path, services: &dyn Services, name: &str) -> Result<()> {
    config_path(dir, name)?;
    // A tunnel that was never started has no service to stop; that is not a failure.
    let _ = services.ctl(name.to_owned(), "disable".to_owned()).await;
    remove_files(dir, name)
}

fn text_failed(error: String) -> TextReply {
    TextReply {
        ok: false,
        error: Some(error),
        text: String::new(),
    }
}

pub async fn logs(name: &str, lines: u32) -> TextReply {
    if !valid_name(name) {
        return text_failed("bad tunnel name".into());
    }
    let args = vec![
        "-u".to_owned(),
        format!("kariz@{name}"),
        "-n".to_owned(),
        lines.clamp(1, 1000).to_string(),
        "--no-pager".to_owned(),
        "-o".to_owned(),
        "short-iso".to_owned(),
    ];
    match run("journalctl", &args, Duration::from_secs(10)).await {
        Ok((true, text)) => TextReply {
            ok: true,
            error: None,
            text,
        },
        Ok((false, text)) => text_failed(text.trim().to_owned()),
        Err(e) => text_failed(format!("{e:#}")),
    }
}

/// The `kariz` program: beside this one, or where the installer puts it.
pub fn kariz_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()
                .map(|d| d.join(format!("kariz{}", std::env::consts::EXE_SUFFIX)))
        })
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin/kariz"))
}

/// Runs the tunnel's speed test through its running daemon (the control socket, as
/// `kariz speedtest` does) and returns the numbers.
pub async fn speedtest(
    dir: &Path,
    name: &str,
    seconds: u32,
    streams: u32,
    udp: bool,
) -> SpeedReply {
    let failed = |error: String| SpeedReply {
        ok: false,
        error: Some(error),
        text: String::new(),
        report: None,
    };
    let path = match config_path(dir, name) {
        Ok(p) => p,
        Err(e) => return failed(format!("{e:#}")),
    };
    let config = match Config::load(&path) {
        Ok(c) if c.role == Role::Entry => c,
        Ok(_) => return failed("the speed test runs on the entry side".into()),
        Err(e) => return failed(format!("{e:#}")),
    };
    let options = kariz::speedtest::Options {
        seconds: u64::from(seconds.clamp(1, 60)),
        streams: streams.clamp(1, 16) as usize,
        udp,
    };
    let limit = Duration::from_secs(options.seconds * 3 + 40);
    match tokio::time::timeout(limit, measure(&config, options)).await {
        Ok(Ok(report)) => SpeedReply {
            ok: true,
            error: None,
            text: String::new(),
            report: Some(report),
        },
        Ok(Err(e)) => failed(format!("{e:#}")),
        Err(_) => failed("the speed test took too long".into()),
    }
}

#[cfg(unix)]
async fn measure(
    config: &Config,
    options: kariz::speedtest::Options,
) -> Result<kariz::speedtest::Report> {
    let socket = config
        .control_socket()
        .context("this tunnel has no control socket")?;
    let connection = kariz::control::connect(&socket)
        .await
        .context("the tunnel is not running")?;
    Ok(kariz::control::request(connection, options, |_: &str| {}).await?)
}

#[cfg(not(unix))]
async fn measure(_: &Config, _: kariz::speedtest::Options) -> Result<kariz::speedtest::Report> {
    bail!("the speed test needs a Linux server")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> Spec {
        Spec {
            name: name.into(),
            role: "entry".into(),
            mode: "reverse".into(),
            transport: "tcpmux".into(),
            listen: Some("0.0.0.0:3080".into()),
            token: Some("a".repeat(48)),
            forwards: vec![ForwardInfo {
                listen: "0.0.0.0:8443".into(),
                target: "127.0.0.1:443".into(),
                protocol: "tcp".into(),
            }],
            ..Default::default()
        }
    }

    /// A directory that goes away with the test.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let mut b = [0u8; 8];
            getrandom::fill(&mut b).unwrap();
            let p = std::env::temp_dir().join(format!(
                "kariz-manage-{}",
                b.iter().map(|x| format!("{x:02x}")).collect::<String>()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_are_strict() {
        for good in ["a", "main", "tehran-1", "0x"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "A", "a b", "../x", "a/b", "a.b", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_spec_makes_a_config_that_reads_back() {
        let d = Dir::new();
        put(&d.0, &spec("main")).unwrap();
        let got = get(&d.0, "main").unwrap();
        assert_eq!(got.role, "entry");
        assert_eq!(got.transport, "tcpmux");
        assert_eq!(got.listen.as_deref(), Some("0.0.0.0:3080"));
        assert_eq!(got.forwards.len(), 1);
        assert!(got.token.is_none(), "the token is never handed back");
        assert_eq!(existing_token(&d.0.join("main.toml")), Some("a".repeat(48)));
    }

    #[test]
    fn an_edit_without_a_token_keeps_the_token_and_the_old_file() {
        let d = Dir::new();
        put(&d.0, &spec("main")).unwrap();
        let mut edit = spec("main");
        edit.token = None;
        edit.listen = Some("0.0.0.0:3081".into());
        put(&d.0, &edit).unwrap();
        assert_eq!(existing_token(&d.0.join("main.toml")), Some("a".repeat(48)));
        assert!(std::fs::read_to_string(d.0.join("main.toml.bak"))
            .unwrap()
            .contains("3080"));
        let mut fresh = spec("other");
        fresh.token = None;
        assert!(put(&d.0, &fresh).is_err(), "a new tunnel needs a token");
    }

    #[test]
    fn values_cannot_write_lines_into_the_file() {
        let d = Dir::new();
        let mut s = spec("main");
        s.listen = Some("0.0.0.0:3080\"\n[control]\nsocket = \"/etc/passwd".into());
        assert!(put(&d.0, &s).is_err());
        assert!(!d.0.join("main.toml").exists());
    }

    #[test]
    fn a_bad_spec_is_refused_with_a_reason() {
        let d = Dir::new();
        let mut s = spec("main");
        s.role = "root".into();
        let r = check(&d.0, &[], &s);
        assert!(!r.ok && r.error.is_some());
        assert!(put(&d.0, &s).is_err());
        assert!(put(&d.0, &spec("../evil")).is_err());
        let mut short = spec("main");
        short.token = Some("abc".into());
        assert!(put(&d.0, &short).is_err());
    }

    #[test]
    fn the_wss_side_and_its_pin_render() {
        let d = Dir::new();
        let s = Spec {
            name: "secure".into(),
            role: "entry".into(),
            mode: "direct".into(),
            transport: "wss".into(),
            remote: Some("203.0.113.5:443".into()),
            token: Some("b".repeat(48)),
            ws_path: Some("/x".into()),
            tls_pin: Some("ab".repeat(32)),
            forwards: vec![ForwardInfo {
                listen: "0.0.0.0:2222".into(),
                target: "127.0.0.1:22".into(),
                protocol: "tcp".into(),
            }],
            ..Default::default()
        };
        put(&d.0, &s).unwrap();
        let got = get(&d.0, "secure").unwrap();
        assert_eq!(got.ws_path.as_deref(), Some("/x"));
        assert_eq!(got.tls_pin, Some("ab".repeat(32)));
    }

    #[test]
    fn a_listening_wss_side_gets_its_own_certificate_and_reports_the_pin() {
        let d = Dir::new();
        let s = Spec {
            name: "secure-exit".into(),
            role: "exit".into(),
            mode: "direct".into(),
            transport: "wss".into(),
            listen: Some("0.0.0.0:443".into()),
            token: Some("d".repeat(48)),
            ws_path: Some("/x".into()),
            ..Default::default()
        };
        let pin = put(&d.0, &s).unwrap().expect("a pin");
        assert_eq!(pin.len(), 64);
        assert!(d.0.join("secure-exit.crt").exists() && d.0.join("secure-exit.key").exists());
        // the same certificate on an edit, and the pin can be read back for the other side
        assert_eq!(put(&d.0, &s).unwrap(), Some(pin.clone()));
        assert_eq!(get(&d.0, "secure-exit").unwrap().tls_pin, Some(pin));
        remove_files(&d.0, "secure-exit").unwrap();
        assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 0);
    }

    #[test]
    fn ports_a_spec_wants() {
        let mut s = spec("main");
        s.forwards.push(ForwardInfo {
            listen: "[::]:5000".into(),
            target: "127.0.0.1:5000".into(),
            protocol: "tcp+udp".into(),
        });
        assert_eq!(
            wanted_ports(&s),
            vec![("tcp", 3080), ("tcp", 8443), ("tcp", 5000), ("udp", 5000)]
        );
        s.transport = "kcp".into();
        assert_eq!(wanted_ports(&s)[0], ("udp", 3080));
        let mut exit = spec("main");
        exit.role = "exit".into();
        exit.forwards.clear();
        assert!(
            wanted_ports(&exit).is_empty(),
            "an exit dials in reverse mode"
        );
    }

    fn owner(proto: &str, port: u16, process: &str) -> PortOwner {
        PortOwner {
            proto: proto.into(),
            addr: "0.0.0.0".into(),
            port,
            process: Some(process.into()),
            pid: Some(1),
        }
    }

    #[test]
    fn a_taken_port_is_named_but_the_tunnels_own_is_not_a_conflict() {
        let d = Dir::new();
        let owners = [owner("tcp", 8443, "nginx"), owner("udp", 3080, "wg")];
        let r = check(&d.0, &owners, &spec("main"));
        assert!(!r.ok);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].process.as_deref(), Some("nginx"));
        // once the tunnel exists, its own listening ports are not a conflict on an edit
        put(&d.0, &spec("main")).unwrap();
        let held = [owner("tcp", 8443, "kariz"), owner("tcp", 3080, "kariz")];
        assert!(check(&d.0, &held, &spec("main")).ok);
        assert!(!check(&d.0, &held, &spec("second")).ok);
    }

    #[test]
    fn proc_net_lines_are_read() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0\n\
   1: 0100007F:1F91 0100007F:9999 01 00000000:00000000 00:00000000 00000000     0        0 555 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(
            parse_proc_net(tcp, false),
            vec![("127.0.0.1".to_string(), 8080, 12345)]
        );
        let tcp6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000001000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 777 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(
            parse_proc_net(tcp6, false),
            vec![("::1".to_string(), 22, 777)]
        );
        let udp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n\
   0: 00000000:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 999 2 0000000000000000 0\n";
        assert_eq!(
            parse_proc_net(udp, true),
            vec![("0.0.0.0".to_string(), 53, 999)]
        );
    }

    #[test]
    fn service_actions_are_a_short_list() {
        assert_eq!(
            ctl_args("main", "restart").unwrap(),
            vec!["restart", "kariz@main"]
        );
        assert_eq!(
            ctl_args("main", "enable").unwrap(),
            vec!["enable", "--now", "kariz@main"]
        );
        assert!(ctl_args("main", "mask").is_err());
        assert!(ctl_args("main; reboot", "start").is_err());
        assert!(ctl_args("a@b", "start").is_err());
    }

    #[test]
    fn deleting_removes_every_file() {
        let d = Dir::new();
        put(&d.0, &spec("main")).unwrap();
        put(&d.0, &spec("main")).unwrap();
        std::fs::write(d.0.join("main.sock"), b"").unwrap();
        remove_files(&d.0, "main").unwrap();
        assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 0);
        assert!(remove_files(&d.0, "main").is_ok(), "twice is fine");
    }
}
