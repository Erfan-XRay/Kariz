//! The benchmark: which transport gets through best between two servers, before a tunnel
//! is built between them (or to see whether a tunnel has a better choice).
//!
//! It takes about 20 seconds, in two stages:
//!
//! 1. **Probe.** For each candidate (a transport, a path: the public address or a GRE link
//!    between the two, and a direction: which server listens) one server listens for a test
//!    session ([`Request::BenchListen`]) and the other dials it ([`Request::BenchDial`]): did
//!    it connect, how long the handshake took, round trips for a second. Candidates are
//!    grouped in lanes, one per listening server and per kind of port (TCP or UDP): a lane's
//!    candidates take turns on the same port (the one the tunnel would use), the lanes run
//!    side by side.
//! 2. **Speed.** The best three of the probe (different transports first) move data each way
//!    for two seconds, one after the other.
//!
//! Every candidate gets a score from 0 to 100: speed, latency and stability, weighted by the
//! tunnel's profile (gaming cares most about latency, ultraspeed about speed). The last
//! result of each pair of servers is kept, so the wizard can show it without running again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use kariz::config::TransportKind;
use kariz::speedtest::Latency;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::auth::now;
use crate::config::random_hex;
use crate::hub::Hub;
use crate::wire::{BenchListening, BenchMeasured, Request};

/// How long a listener waits for its dialer.
const LISTEN_WAIT: Duration = Duration::from_secs(5);
/// How long a dialer waits for its connection and handshake (a blocked transport fails here).
const DIAL_LIMIT: Duration = Duration::from_secs(3);
/// Round trips measured in the probe stage, and in the speed stage before data moves.
const PROBE_PING: Duration = Duration::from_millis(1000);
const SPEED_PING: Duration = Duration::from_millis(300);
/// Seconds of data each way in the speed stage, and on how many streams.
const SPEED_SECONDS: u32 = 2;
const SPEED_STREAMS: usize = 4;
/// Candidates that get the speed stage.
const SPEED_TOP: usize = 3;
/// Test listeners an agent holds at once.
const MAX_LISTENERS: usize = 6;
/// The whole run, whatever happens.
const BUDGET: Duration = Duration::from_secs(60);

// ---- the agent's half ----

/// The agent's test listeners that are up now.
#[derive(Default)]
pub struct Listeners(Arc<AtomicUsize>);

fn kind_of(name: &str) -> Option<TransportKind> {
    kariz::bench::TRANSPORTS
        .into_iter()
        .find(|k| k.name() == name)
}

fn valid_token(token: &str) -> bool {
    (32..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// [`Request::BenchListen`] on an agent: binds (the port asked for, waiting a moment for
/// the previous test of the lane to let it go, else any free port) and answers one test
/// session in the background. `tls` is the certificate a `wss` listener shows.
pub async fn listen(
    listeners: &Listeners,
    token: &str,
    transport: &str,
    profile: &str,
    port: u16,
    wait_ms: u32,
    tls: (&std::path::Path, &std::path::Path),
) -> BenchListening {
    let failed = |e: String| BenchListening {
        ok: false,
        error: Some(e),
        port: 0,
    };
    let Some(kind) = kind_of(transport) else {
        return failed(format!("unknown transport {transport:?}"));
    };
    if !valid_token(token) || wait_ms == 0 || wait_ms > 15_000 {
        return failed("bad_input".into());
    }
    let live = listeners.0.clone();
    if live.fetch_add(1, Ordering::SeqCst) >= MAX_LISTENERS {
        live.fetch_sub(1, Ordering::SeqCst);
        return failed("busy".into());
    }
    let tls = if kind == TransportKind::Wss {
        if let Err(e) = crate::cert::ensure(tls.0, tls.1) {
            live.fetch_sub(1, Ordering::SeqCst);
            return failed(format!("{e:#}"));
        }
        Some(tls)
    } else {
        None
    };
    let mut bound = Err(std::io::Error::other("not bound"));
    if port != 0 {
        for _ in 0..10 {
            bound =
                kariz::bench::listen(&format!("0.0.0.0:{port}"), token, kind, profile, tls).await;
            if bound.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    if bound.is_err() {
        bound = kariz::bench::listen("0.0.0.0:0", token, kind, profile, tls).await;
    }
    let acceptor = match bound {
        Ok(a) => a,
        Err(e) => {
            live.fetch_sub(1, Ordering::SeqCst);
            return failed(e.to_string());
        }
    };
    let port = acceptor.local_addr().map(|a| a.port()).unwrap_or(0);
    tokio::spawn(async move {
        let result =
            kariz::bench::answer(acceptor, Duration::from_millis(u64::from(wait_ms))).await;
        if let Err(e) = result {
            debug!(error = %e, "a benchmark listener ended");
        }
        live.fetch_sub(1, Ordering::SeqCst);
    });
    BenchListening {
        ok: true,
        error: None,
        port,
    }
}

/// [`Request::BenchDial`] on an agent: dials the test listener and measures the link.
pub async fn dial(
    addr: &str,
    token: &str,
    transport: &str,
    profile: &str,
    ping_ms: u32,
    seconds: u32,
) -> BenchMeasured {
    let failed = |e: String| BenchMeasured {
        ok: false,
        error: Some(e),
        ..Default::default()
    };
    let Some(kind) = kind_of(transport) else {
        return failed(format!("unknown transport {transport:?}"));
    };
    if !valid_token(token) || ping_ms > 5_000 || seconds > 5 || !crate::join::valid_host(addr) {
        return failed("bad_input".into());
    }
    let (session, connect_ms) =
        match kariz::bench::dial(addr, token, kind, profile, DIAL_LIMIT).await {
            Ok(v) => v,
            Err(e) => return failed(e.to_string()),
        };
    let mut out = BenchMeasured {
        ok: true,
        connect_ms,
        ..Default::default()
    };
    if ping_ms > 0 {
        match kariz::bench::probe(&session, Duration::from_millis(u64::from(ping_ms))).await {
            Ok(latency) if latency.received > 0 => out.latency = Some(latency),
            Ok(_) => {
                out.ok = false;
                out.error = Some("connected, but nothing came back".into());
            }
            Err(e) => {
                out.ok = false;
                out.error = Some(e.to_string());
            }
        }
    }
    if out.ok && seconds > 0 {
        match kariz::bench::speed(&session, u64::from(seconds), SPEED_STREAMS).await {
            Ok(speed) => {
                out.download_mbps = Some(speed.download.mbps);
                out.upload_mbps = Some(speed.upload.mbps);
                out.loaded = Some(speed.loaded);
            }
            Err(e) => {
                out.ok = false;
                out.error = Some(e.to_string());
            }
        }
    }
    session.close();
    out
}

// ---- the panel's half ----

/// One thing the benchmark measures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    /// `mode:path:transport`, unique in a run.
    pub key: String,
    /// The core transport (`tcpmux`, `kcp`, `ws`, `wss`, `quic`).
    pub transport: String,
    /// `public`, or `gre` (across the private network link between the two).
    pub path: String,
    /// The GRE link's network (id), for `gre`.
    pub network: Option<String>,
    /// `reverse` (the entry listens) or `direct` (the exit listens).
    pub mode: String,
    /// The server that listens.
    pub listener: String,
    /// What was dialed: the listening server's address (and the port it got).
    pub host: String,
    pub port: Option<u16>,
    /// `wait`, `probe`, `speed`, `ok`, `fail`.
    pub state: String,
    pub connect_ms: Option<f64>,
    pub latency: Option<Latency>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub loaded: Option<Latency>,
    pub score: Option<u32>,
    pub parts: Option<Parts>,
    pub error: Option<String>,
}

/// What a score is made of, each 0 to 100.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Parts {
    pub speed: u32,
    pub latency: u32,
    pub stability: u32,
}

/// One run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bench {
    pub id: String,
    pub entry: String,
    pub exit: String,
    pub profile: String,
    /// `probe`, `speed`, `done`, `stopped` or `failed`.
    pub state: String,
    /// Unix milliseconds.
    pub started: i64,
    pub finished: Option<i64>,
    pub candidates: Vec<Candidate>,
    /// The best candidate's key.
    pub best: Option<String>,
    pub error: Option<String>,
}

/// The runs of the last while, and the stop switches of the running ones.
#[derive(Default)]
pub struct Runs {
    runs: Mutex<Vec<Bench>>,
    stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Runs {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Bench>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Bench> {
        self.lock().iter().find(|b| b.id == id).cloned()
    }

    fn with(&self, id: &str, f: impl FnOnce(&mut Bench)) {
        if let Some(b) = self.lock().iter_mut().find(|b| b.id == id) {
            f(b);
        }
    }

    fn candidate(&self, id: &str, key: &str, f: impl FnOnce(&mut Candidate)) {
        self.with(id, |b| {
            if let Some(c) = b.candidates.iter_mut().find(|c| c.key == key) {
                f(c);
            }
        });
    }

    /// Stops a run at its next step. False when it is not running.
    pub fn stop(&self, id: &str) -> bool {
        match self.stops.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

/// What a run is asked to measure.
#[derive(Debug, Clone, Deserialize)]
pub struct Ask {
    pub entry: String,
    pub exit: String,
    #[serde(default = "balanced")]
    pub profile: String,
    /// The port the tunnel would listen on (tried first, so a filter on it shows); none or
    /// 0: any.
    #[serde(default)]
    pub port: u16,
    /// `reverse`, `direct`, or both (none: both).
    #[serde(default)]
    pub modes: Vec<String>,
    /// Where to dial a server instead of its address (the tests, on one machine: a
    /// loopback address is never a server's address). Not read from a request.
    #[serde(skip)]
    pub hosts: HashMap<String, String>,
}

fn balanced() -> String {
    "balanced".into()
}

fn is_udp(transport: &str) -> bool {
    matches!(transport, "kcp" | "quic")
}

/// The candidates for `ask`: every transport in each direction over the public addresses,
/// and `tcpmux` and `kcp` across a GRE link if the two servers have one (a private link is
/// not filtered: the transports differ there only by their own cost).
fn candidates(hub: &Hub, ask: &Ask) -> Result<Vec<Candidate>> {
    let links = hub.networks.links(None)?;
    let link = links
        .iter()
        .find(|l| (l.a == ask.entry && l.b == ask.exit) || (l.a == ask.exit && l.b == ask.entry));
    let modes: Vec<&str> = if ask.modes.is_empty() {
        vec!["reverse", "direct"]
    } else {
        ask.modes.iter().map(String::as_str).collect()
    };
    let mut out = Vec::new();
    for mode in modes {
        let listener = match mode {
            "reverse" => &ask.entry,
            "direct" => &ask.exit,
            _ => bail!("bad_input"),
        };
        let public = ask
            .hosts
            .get(listener)
            .cloned()
            .or_else(|| hub.addr_of(listener))
            .unwrap_or_default();
        let mut add = |transport: &str, path: &str, host: String, network: Option<String>| {
            out.push(Candidate {
                key: format!("{mode}:{path}:{transport}"),
                transport: transport.into(),
                path: path.into(),
                network,
                mode: mode.into(),
                listener: listener.clone(),
                host,
                port: None,
                state: "wait".into(),
                connect_ms: None,
                latency: None,
                download_mbps: None,
                upload_mbps: None,
                loaded: None,
                score: None,
                parts: None,
                error: None,
            });
        };
        for kind in kariz::bench::TRANSPORTS {
            add(kind.name(), "public", public.clone(), None);
        }
        if let Some(l) = link {
            let private = if &l.a == listener {
                &l.addr_a
            } else {
                &l.addr_b
            };
            for transport in ["tcpmux", "kcp"] {
                add(transport, "gre", private.clone(), Some(l.network.clone()));
            }
        }
    }
    Ok(out)
}

/// Starts a run; its id comes back at once, and [`Runs::get`] follows it. Errors: `bad_input`,
/// `same_server`, `offline:ID`, `busy`.
pub fn start(hub: &Arc<Hub>, ask: Ask) -> Result<String> {
    if ask.entry == ask.exit {
        bail!("same_server");
    }
    if !matches!(ask.profile.as_str(), "balanced" | "ultraspeed" | "gaming") {
        bail!("bad_input");
    }
    let views = hub.snapshot()?;
    for id in [&ask.entry, &ask.exit] {
        match views.iter().find(|s| &s.id == id) {
            // The panel's own server is always there: its agent runs inside the panel.
            Some(s) if s.online || s.local => {}
            Some(_) => bail!("offline:{id}"),
            None => bail!("bad_input"),
        }
    }
    let candidates = candidates(hub, &ask)?;
    let id = random_hex(8)?;
    {
        let mut runs = hub.bench.lock();
        let pair = |b: &Bench| {
            (b.entry == ask.entry && b.exit == ask.exit)
                || (b.entry == ask.exit && b.exit == ask.entry)
        };
        if runs
            .iter()
            .any(|b| pair(b) && matches!(b.state.as_str(), "probe" | "speed"))
        {
            bail!("busy");
        }
        if runs.len() >= 20 {
            runs.remove(0);
        }
        runs.push(Bench {
            id: id.clone(),
            entry: ask.entry.clone(),
            exit: ask.exit.clone(),
            profile: ask.profile.clone(),
            state: "probe".into(),
            started: now() * 1000,
            finished: None,
            candidates,
            best: None,
            error: None,
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    hub.bench
        .stops
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), stop.clone());
    let (hub, run) = (hub.clone(), id.clone());
    tokio::spawn(async move {
        let token = random_hex(32).unwrap_or_default();
        let ran = tokio::time::timeout(BUDGET, run_all(&hub, &run, &token, &ask, &stop)).await;
        let stopped = stop.load(Ordering::SeqCst);
        hub.bench.with(&run, |b| {
            score(b);
            for c in &mut b.candidates {
                if matches!(c.state.as_str(), "wait" | "probe" | "speed") && c.score.is_none() {
                    c.state = "skip".into();
                }
            }
            b.finished = Some(now() * 1000);
            b.state = match ran {
                Err(_) => {
                    b.error = Some("timeout".into());
                    "done"
                }
                Ok(Err(ref e)) => {
                    b.error = Some(format!("{e:#}"));
                    "failed"
                }
                Ok(Ok(())) if stopped => "stopped",
                Ok(Ok(())) => "done",
            }
            .into();
        });
        hub.bench
            .stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&run);
        if let Some(done) = hub.bench.get(&run) {
            if done.state == "done" || done.state == "stopped" {
                let _ = save(&hub, &done);
            }
        }
    });
    Ok(id)
}

async fn run_all(
    hub: &Arc<Hub>,
    id: &str,
    token: &str,
    ask: &Ask,
    stop: &AtomicBool,
) -> Result<()> {
    let bench = hub.bench.get(id).ok_or_else(|| anyhow!("gone"))?;
    // Stage 1: lanes side by side, each one's candidates in turn.
    let mut lanes: Vec<(String, bool, Vec<Candidate>)> = Vec::new();
    for c in bench.candidates {
        let udp = is_udp(&c.transport);
        match lanes
            .iter_mut()
            .find(|(l, u, _)| *l == c.listener && *u == udp)
        {
            Some(lane) => lane.2.push(c),
            None => lanes.push((c.listener.clone(), udp, vec![c])),
        }
    }
    let mut running = tokio::task::JoinSet::new();
    for (_, _, lane) in lanes {
        let (hub, id, token, ask) = (hub.clone(), id.to_owned(), token.to_owned(), ask.clone());
        let stop = hub
            .bench
            .stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned();
        running.spawn(async move {
            let mut port = ask.port;
            for c in lane {
                if stop.as_ref().is_some_and(|s| s.load(Ordering::SeqCst)) {
                    return;
                }
                let dialer = if c.listener == ask.entry {
                    &ask.exit
                } else {
                    &ask.entry
                };
                hub.bench
                    .candidate(&id, &c.key, |x| x.state = "probe".into());
                let got =
                    measure(&hub, &c, dialer, &token, &ask.profile, port, PROBE_PING, 0).await;
                if let Some(p) = got.0 {
                    // The lane keeps the port its first listener got.
                    port = p;
                }
                hub.bench.candidate(&id, &c.key, |x| apply(x, got));
            }
        });
    }
    while running.join_next().await.is_some() {}
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Stage 2: the best few of the probe, different transports first, one after the other.
    hub.bench.with(id, |b| {
        b.state = "speed".into();
        score(b);
    });
    let probed = hub.bench.get(id).ok_or_else(|| anyhow!("gone"))?;
    let mut ranked: Vec<&Candidate> = probed
        .candidates
        .iter()
        .filter(|c| c.state == "ok")
        .collect();
    ranked.sort_by_key(|c| std::cmp::Reverse(c.score));
    let mut picked: Vec<Candidate> = Vec::new();
    for c in &ranked {
        if picked.len() < SPEED_TOP && !picked.iter().any(|p| p.transport == c.transport) {
            picked.push((*c).clone());
        }
    }
    for c in &ranked {
        if picked.len() < SPEED_TOP && !picked.iter().any(|p| p.key == c.key) {
            picked.push((*c).clone());
        }
    }
    for c in picked {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let dialer = if c.listener == ask.entry {
            &ask.exit
        } else {
            &ask.entry
        };
        hub.bench
            .candidate(id, &c.key, |x| x.state = "speed".into());
        let port = c.port.unwrap_or(ask.port);
        let got = measure(
            hub,
            &c,
            dialer,
            token,
            &ask.profile,
            port,
            SPEED_PING,
            SPEED_SECONDS,
        )
        .await;
        hub.bench.candidate(id, &c.key, |x| {
            let (_, m) = &got;
            if m.ok {
                x.download_mbps = m.download_mbps;
                x.upload_mbps = m.upload_mbps;
                x.loaded = m.loaded.clone();
                x.state = "ok".into();
            } else {
                // It got through the probe but not this: it is not reliable.
                x.state = "fail".into();
                x.error = m.error.clone();
            }
        });
        hub.bench.with(id, score);
    }
    Ok(())
}

/// One candidate: a listener on one server, a dialer on the other.
#[allow(clippy::too_many_arguments)]
async fn measure(
    hub: &Hub,
    c: &Candidate,
    dialer: &str,
    token: &str,
    profile: &str,
    port: u16,
    ping: Duration,
    seconds: u32,
) -> (Option<u16>, BenchMeasured) {
    let failed = |e: String| BenchMeasured {
        ok: false,
        error: Some(e),
        ..Default::default()
    };
    if c.host.is_empty() {
        return (None, failed(format!("no_address:{}", c.listener)));
    }
    let listening: BenchListening = match hub
        .ask_as_within(
            &c.listener,
            &Request::BenchListen {
                token: token.into(),
                transport: c.transport.clone(),
                profile: profile.into(),
                port,
                wait_ms: LISTEN_WAIT.as_millis() as u32,
            },
            Duration::from_secs(10),
        )
        .await
    {
        Ok(l) => l,
        Err(e) => return (None, failed(agent_error(&e))),
    };
    if !listening.ok {
        return (
            None,
            failed(listening.error.unwrap_or_else(|| "failed".into())),
        );
    }
    let host = if c.host.contains(':') {
        format!("[{}]", c.host)
    } else {
        c.host.clone()
    };
    let measured: BenchMeasured = match hub
        .ask_as_within(
            dialer,
            &Request::BenchDial {
                addr: format!("{host}:{}", listening.port),
                token: token.into(),
                transport: c.transport.clone(),
                profile: profile.into(),
                ping_ms: ping.as_millis() as u32,
                seconds,
            },
            Duration::from_secs(20),
        )
        .await
    {
        Ok(m) => m,
        Err(e) => failed(agent_error(&e)),
    };
    (Some(listening.port), measured)
}

/// An agent that does not know the benchmark answers with something else.
fn agent_error(e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    if text.contains("unknown variant")
        || text.contains("not understood")
        || text.contains("expected")
    {
        "agent_old".into()
    } else {
        text
    }
}

fn apply(c: &mut Candidate, (port, m): (Option<u16>, BenchMeasured)) {
    c.port = port.or(c.port);
    c.connect_ms = (m.connect_ms > 0.0).then_some(m.connect_ms);
    c.latency = m.latency;
    if m.ok {
        c.state = "ok".into();
    } else {
        c.state = "fail".into();
        c.error = m.error;
    }
}

/// How much each part counts for a profile: speed, latency, stability.
fn weights(profile: &str) -> (f64, f64, f64) {
    match profile {
        "gaming" => (0.20, 0.55, 0.25),
        "ultraspeed" => (0.60, 0.15, 0.25),
        _ => (0.40, 0.30, 0.30),
    }
}

/// Scores every candidate that got through, against the best of the run.
///
/// * speed: download and upload (7:3) against the best, on a square-root scale (half the
///   speed scores 71); 0 when not measured;
/// * latency: the middle round trip plus jitter, as `(best + 40) / (this + 40)`: a few
///   milliseconds more than the best costs little, twice as slow costs a lot;
/// * stability: lost round trips (each percent costs 3), a slow handshake, and how many
///   milliseconds the latency grows while data moves (bufferbloat: 200 ms more costs 30).
///
/// Once some candidates have been measured for speed, only they get a score: the others got
/// through but were not measured the same way, and are listed apart.
pub fn score(b: &mut Bench) {
    let (ws, wl, wst) = weights(&b.profile);
    let ok: Vec<&Candidate> = b.candidates.iter().filter(|c| c.state == "ok").collect();
    let speed_of = |c: &Candidate| match (c.download_mbps, c.upload_mbps) {
        (Some(d), Some(u)) => Some(0.7 * d + 0.3 * u),
        _ => None,
    };
    let lat_of = |c: &Candidate| c.latency.as_ref().map(|l| l.p50_ms + l.jitter_ms);
    let best_speed = ok.iter().filter_map(|c| speed_of(c)).fold(0.0f64, f64::max);
    let measured = best_speed > 0.0;
    let best_lat = ok
        .iter()
        .filter_map(|c| lat_of(c))
        .fold(f64::INFINITY, f64::min);
    let scored: Vec<(String, Parts, u32)> = ok
        .iter()
        .filter(|c| !measured || speed_of(c).is_some())
        .map(|c| {
            let speed = match speed_of(c) {
                Some(s) if best_speed > 0.0 => (s / best_speed).sqrt(),
                _ => 0.0,
            };
            let latency = match lat_of(c) {
                Some(l) if best_lat.is_finite() => (best_lat + 40.0) / (l + 40.0),
                _ => 0.0,
            };
            let mut stability = 1.0;
            if let Some(l) = &c.latency {
                stability -= l.loss_percent() * 0.03;
            }
            if c.connect_ms.is_some_and(|ms| ms > 1500.0) {
                stability -= 0.1;
            }
            if let (Some(idle), Some(loaded)) = (&c.latency, &c.loaded) {
                stability -= ((loaded.p50_ms - idle.p50_ms) / 200.0 * 0.3).clamp(0.0, 0.3);
                stability -= loaded.loss_percent() * 0.03;
            }
            let (speed, latency, stability) = (
                speed.clamp(0.0, 1.0),
                latency.clamp(0.0, 1.0),
                stability.clamp(0.0, 1.0),
            );
            let total = (100.0 * (ws * speed + wl * latency + wst * stability)).round() as u32;
            let pct = |x: f64| (x * 100.0).round() as u32;
            (
                c.key.clone(),
                Parts {
                    speed: pct(speed),
                    latency: pct(latency),
                    stability: pct(stability),
                },
                total,
            )
        })
        .collect();
    for c in &mut b.candidates {
        match scored.iter().find(|(k, ..)| *k == c.key) {
            Some((_, parts, total)) => {
                c.parts = Some(*parts);
                c.score = Some(*total);
            }
            None => {
                c.parts = None;
                c.score = None;
            }
        }
    }
    b.best = scored
        .iter()
        .max_by_key(|(_, _, total)| *total)
        .map(|(k, ..)| k.clone());
}

/// Keeps a finished run as the last of its pair of servers.
fn save(hub: &Hub, b: &Bench) -> Result<()> {
    hub.db.conn().execute(
        "INSERT INTO benchmarks (entry, exit, created, result) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (entry, exit) DO UPDATE SET created = excluded.created, result = excluded.result",
        params![b.entry, b.exit, now(), serde_json::to_string(b)?],
    )?;
    Ok(())
}

/// The last finished run between `entry` and `exit` (in that order).
pub fn last(hub: &Hub, entry: &str, exit: &str) -> Result<Option<Bench>> {
    let text: Option<String> = hub
        .db
        .conn()
        .query_row(
            "SELECT result FROM benchmarks WHERE entry = ?1 AND exit = ?2",
            [entry, exit],
            |r| r.get(0),
        )
        .optional()?;
    Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(key: &str, state: &str, p50: f64, down: Option<f64>) -> Candidate {
        Candidate {
            key: key.into(),
            transport: key.into(),
            path: "public".into(),
            network: None,
            mode: "reverse".into(),
            listener: "a".into(),
            host: "198.51.100.1".into(),
            port: Some(3080),
            state: state.into(),
            connect_ms: Some(120.0),
            latency: Some(Latency {
                sent: 10,
                received: 10,
                p50_ms: p50,
                p99_ms: p50 * 1.5,
                jitter_ms: 2.0,
            }),
            download_mbps: down,
            upload_mbps: down.map(|d| d / 4.0),
            loaded: None,
            score: None,
            parts: None,
            error: None,
        }
    }

    fn bench(profile: &str, candidates: Vec<Candidate>) -> Bench {
        Bench {
            id: "x".into(),
            entry: "a".into(),
            exit: "b".into(),
            profile: profile.into(),
            state: "done".into(),
            started: 0,
            finished: None,
            candidates,
            best: None,
            error: None,
        }
    }

    #[test]
    fn the_profile_decides_between_fast_and_quick() {
        // kcp: a little slower but much quicker; tcpmux: the fastest, slower round trips.
        let make = |p| {
            bench(
                p,
                vec![
                    cand("tcpmux", "ok", 160.0, Some(900.0)),
                    cand("kcp", "ok", 60.0, Some(500.0)),
                    cand("ws", "fail", 0.0, None),
                ],
            )
        };
        let mut gaming = make("gaming");
        score(&mut gaming);
        assert_eq!(gaming.best.as_deref(), Some("kcp"));
        let mut fast = make("ultraspeed");
        score(&mut fast);
        assert_eq!(fast.best.as_deref(), Some("tcpmux"));
        // What failed has no score.
        assert!(fast.candidates[2].score.is_none());
        assert!(fast.candidates.iter().all(|c| c.score.unwrap_or(0) <= 100));
    }

    #[test]
    fn speed_not_measured_ranks_below_measured() {
        let mut b = bench(
            "balanced",
            vec![
                cand("tcpmux", "ok", 80.0, Some(300.0)),
                cand("quic", "ok", 80.0, None),
            ],
        );
        score(&mut b);
        assert_eq!(b.best.as_deref(), Some("tcpmux"));
        // It got through, but is not ranked against what was measured.
        assert!(b.candidates[1].score.is_none());
        // Before any speed is measured, everything that got through is ranked.
        b.candidates[0].download_mbps = None;
        score(&mut b);
        assert!(b.candidates.iter().all(|c| c.score.is_some()));
    }

    #[test]
    fn bufferbloat_counts_milliseconds_not_ratios() {
        let mut lan = cand("tcpmux", "ok", 0.3, Some(3000.0));
        lan.loaded = Some(Latency {
            sent: 10,
            received: 10,
            p50_ms: 3.0,
            p99_ms: 4.0,
            jitter_ms: 0.5,
        });
        let mut bloated = cand("kcp", "ok", 80.0, Some(3000.0));
        bloated.loaded = Some(Latency {
            sent: 10,
            received: 10,
            p50_ms: 280.0,
            p99_ms: 400.0,
            jitter_ms: 9.0,
        });
        let mut b = bench("balanced", vec![lan, bloated]);
        score(&mut b);
        assert!(b.candidates[0].parts.unwrap().stability >= 99);
        assert_eq!(b.candidates[1].parts.unwrap().stability, 70);
    }

    #[test]
    fn lost_round_trips_cost_stability() {
        let mut lossy = cand("kcp", "ok", 80.0, Some(300.0));
        lossy.latency.as_mut().unwrap().received = 8;
        let mut b = bench(
            "balanced",
            vec![cand("tcpmux", "ok", 80.0, Some(300.0)), lossy],
        );
        score(&mut b);
        let (clean, lossy) = (
            b.candidates[0].parts.unwrap(),
            b.candidates[1].parts.unwrap(),
        );
        assert_eq!(clean.stability, 100);
        assert_eq!(lossy.stability, 40);
        assert_eq!(b.best.as_deref(), Some("tcpmux"));
    }

    #[test]
    fn tokens_and_transports_are_checked() {
        assert!(valid_token(&"ab".repeat(32)));
        assert!(!valid_token("short"));
        assert!(!valid_token(&"zz".repeat(32)));
        assert_eq!(kind_of("kcp"), Some(TransportKind::Kcp));
        assert_eq!(kind_of("tcp"), None);
        assert_eq!(kind_of("auto"), None);
    }
}
