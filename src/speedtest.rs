//! A speed test through the live tunnel: what a user's traffic really gets, with the
//! transport, encryption, mux and profile in use.
//!
//! The entry side opens test streams to the exit side ([`KIND_SPEEDTEST`]
//! (crate::proto::KIND_SPEEDTEST) opens, the target says what to do) and measures:
//!
//! * `down`: the exit sends data as fast as it can, until the entry hangs up;
//! * `up`: the exit reads and discards;
//! * `echo`: the exit sends back what it gets: latency, idle and under load;
//! * `udp`: the exit echoes datagrams (mux sessions only): loss and jitter of the
//!   datagram path UDP forwarding uses.
//!
//! The exit dials nothing and keeps nothing: a test stream ends when the entry ends it,
//! and after [`MAX_STREAM`] at the latest.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{interval, sleep_until, timeout, timeout_at, Instant, MissedTickBehavior};

use crate::channel::{Channel, Link};
use crate::session::SessionStream;

/// Longest a test stream lives on the exit side, whatever the entry does.
pub const MAX_STREAM: Duration = Duration::from_secs(120);
/// Bounds for a test's settings.
pub const MAX_SECONDS: u64 = 60;
pub const MAX_STREAMS: usize = 32;

const CHUNK: usize = 64 * 1024;
const PING_EVERY: Duration = Duration::from_millis(100);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const SAMPLE_EVERY: Duration = Duration::from_millis(250);
const UDP_RATE: u64 = 64;
const UDP_PACKET: usize = 128;
const UDP_GRACE: Duration = Duration::from_millis(1500);
const UDP_MAX_SECONDS: u64 = 5;
/// The exit gives up on a datagram echo stream that has been silent this long.
const UDP_IDLE: Duration = Duration::from_secs(5);

/// What one side of a test stream reads and writes: a session stream or a whole tunnel
/// connection (mux off).
pub enum Pipe {
    Stream(SessionStream),
    Link(Box<Link>, BytesMut),
}

impl From<Channel> for Pipe {
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Stream(s) => Self::Stream(s),
            Channel::Link(l) => Self::Link(Box::new(l), BytesMut::new()),
        }
    }
}

impl Pipe {
    async fn send(&mut self, data: Bytes) -> io::Result<()> {
        match self {
            Self::Stream(s) => s.send(data).await,
            Self::Link(l, _) => {
                l.write_all(&data).await?;
                l.flush().await
            }
        }
    }

    /// The next chunk; `None` at the end of the stream.
    async fn recv(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            Self::Stream(s) => s.recv().await,
            Self::Link(l, buf) => {
                buf.reserve(CHUNK);
                if l.read_buf(buf).await? == 0 {
                    return Ok(None);
                }
                Ok(Some(buf.split().freeze()))
            }
        }
    }

    /// Datagrams need a session stream.
    fn datagrams(&self) -> Option<&SessionStream> {
        match self {
            Self::Stream(s) => Some(s),
            Self::Link(..) => None,
        }
    }
}

/// 64 KiB of data that does not compress, made once.
fn payload() -> Bytes {
    static PAYLOAD: OnceLock<Bytes> = OnceLock::new();
    PAYLOAD
        .get_or_init(|| {
            let mut x = 0x9e37_79b9_7f4a_7c15u64;
            let bytes: Vec<u8> = (0..CHUNK)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    (x >> 24) as u8
                })
                .collect();
            Bytes::from(bytes)
        })
        .clone()
}

// ---- Exit side ----

/// Serves one test stream: `command` is the target of the open request.
pub async fn serve(mut pipe: Pipe, command: &str) -> io::Result<()> {
    let until = Instant::now() + MAX_STREAM;
    match command {
        "down" => {
            let chunk = payload();
            while Instant::now() < until {
                pipe.send(chunk.clone()).await?;
            }
            Ok(())
        }
        "up" => {
            while let Ok(Ok(Some(_))) = timeout_at(until, pipe.recv()).await {}
            Ok(())
        }
        "echo" => {
            while let Ok(Ok(Some(chunk))) = timeout_at(until, pipe.recv()).await {
                pipe.send(chunk).await?;
            }
            Ok(())
        }
        "udp" => {
            let Some(stream) = pipe.datagrams() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "datagram tests need mux",
                ));
            };
            loop {
                let deadline = (Instant::now() + UDP_IDLE).min(until);
                match timeout_at(deadline, stream.recv_datagram()).await {
                    Ok(Ok(Some(datagram))) => {
                        stream.send_datagram(datagram);
                    }
                    _ => return Ok(()),
                }
            }
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown speed test command {other:?}"),
        )),
    }
}

// ---- Entry side ----

/// A test's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Seconds each of the download and upload phases lasts.
    pub seconds: u64,
    /// Parallel streams in those phases.
    pub streams: usize,
    /// Also test UDP (needs mux).
    pub udp: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seconds: 10,
            streams: 4,
            udp: true,
        }
    }
}

impl Options {
    pub fn validate(&self) -> io::Result<()> {
        if !(1..=MAX_SECONDS).contains(&self.seconds) {
            return Err(invalid(format!("seconds must be 1 to {MAX_SECONDS}")));
        }
        if !(1..=MAX_STREAMS).contains(&self.streams) {
            return Err(invalid(format!("streams must be 1 to {MAX_STREAMS}")));
        }
        Ok(())
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Round trips of a series of small messages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    pub sent: u32,
    pub received: u32,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub jitter_ms: f64,
}

impl Latency {
    /// `rtts` in the order the messages were sent (the lost ones are missing).
    fn from_samples(sent: u32, rtts: &[Duration]) -> Self {
        let mut sorted: Vec<f64> = rtts.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        let jitter = if sorted.len() > 1 {
            sorted.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (sorted.len() - 1) as f64
        } else {
            0.0
        };
        sorted.sort_by(f64::total_cmp);
        let pick = |p: f64| {
            if sorted.is_empty() {
                0.0
            } else {
                sorted[((sorted.len() - 1) as f64 * p).round() as usize]
            }
        };
        Self {
            sent,
            received: rtts.len() as u32,
            p50_ms: round1(pick(0.5)),
            p99_ms: round1(pick(0.99)),
            jitter_ms: round1(jitter),
        }
    }

    /// Share of the messages that never came back, in percent.
    pub fn loss_percent(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            100.0 * f64::from(self.sent - self.received.min(self.sent)) / f64::from(self.sent)
        }
    }
}

/// Bytes moved in one direction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    /// Average after the warm-up.
    pub mbps: f64,
    /// Best second.
    pub peak_mbps: f64,
    pub bytes: u64,
}

/// Everything a test measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub seconds: u64,
    pub streams: usize,
    pub idle: Latency,
    pub download: Rate,
    pub download_latency: Latency,
    pub upload: Rate,
    pub upload_latency: Latency,
    /// `None` when UDP was not tested.
    pub udp: Option<Latency>,
    pub notes: Vec<String>,
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn mbps(bytes: u64, over: Duration) -> f64 {
    if over.is_zero() {
        return 0.0;
    }
    round1(bytes as f64 * 8.0 / over.as_secs_f64() / 1e6)
}

/// Prefix of the progress lines meant for programs (the panel), not for a person: `@idle`,
/// `@download`, `@upload` and `@udp`, then the phase's result as JSON. `kariz speedtest` does
/// not print them.
pub const RESULT_LINE: char = '@';

fn result_line(phase: &str, result: &impl Serialize) -> String {
    format!(
        "{RESULT_LINE}{phase} {}",
        serde_json::to_string(result).unwrap_or_default()
    )
}

/// Runs a whole test. `open` opens a test stream for a command (`down`, `up`, `echo`,
/// `udp`) through the tunnel; `progress` gets a line now and then (the phases are
/// `latency, idle`, `download, N streams`, `upload, N streams`, `UDP datagrams`; a rate is
/// `↓ N Mbit/s` or `↑ N Mbit/s`, every half second; a finished phase sends its
/// [`RESULT_LINE`] too).
pub async fn run<O, F>(
    open: O,
    options: Options,
    mut progress: impl FnMut(String),
) -> io::Result<Report>
where
    O: Fn(&'static str) -> F,
    F: std::future::Future<Output = io::Result<Pipe>>,
{
    options.validate()?;
    let mut report = Report {
        seconds: options.seconds,
        streams: options.streams,
        ..Default::default()
    };

    progress("latency, idle".into());
    let idle_for = Duration::from_secs(options.seconds.min(2));
    report.idle = ping_loop(open("echo").await?, Instant::now() + idle_for).await;
    progress(result_line("idle", &report.idle));
    if report.idle.received == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the exit side did not answer the test streams: it is older than v0.6, has \
             `speedtest = false`, or the tunnel is down",
        ));
    }

    progress(format!("download, {} streams", options.streams));
    (report.download, report.download_latency) =
        measure_rate(&open, "down", options, &mut progress, "↓").await?;
    progress(result_line("download", &report.download));
    progress(format!("upload, {} streams", options.streams));
    (report.upload, report.upload_latency) =
        measure_rate(&open, "up", options, &mut progress, "↑").await?;
    progress(result_line("upload", &report.upload));

    if options.udp {
        progress("UDP datagrams".into());
        match open("udp").await {
            Ok(pipe) if pipe.datagrams().is_some() => {
                let udp = udp_test(pipe, options.seconds.min(UDP_MAX_SECONDS)).await;
                progress(result_line("udp", &udp));
                report.udp = Some(udp);
            }
            _ => report
                .notes
                .push("UDP was not tested: it needs mux on this tunnel".into()),
        }
    }
    Ok(report)
}

/// Aborts its tasks when it is dropped: a test that is cut off (its client went away, or
/// pressed stop) must not leave streams pumping data behind it.
struct Reap(Vec<tokio::task::AbortHandle>);

impl Drop for Reap {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// One phase in one direction: `streams` streams moving data for `seconds` while another
/// stream measures latency under that load.
async fn measure_rate<O, F>(
    open: &O,
    command: &'static str,
    options: Options,
    progress: &mut impl FnMut(String),
    arrow: &str,
) -> io::Result<(Rate, Latency)>
where
    O: Fn(&'static str) -> F,
    F: std::future::Future<Output = io::Result<Pipe>>,
{
    let moved = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let end = start + Duration::from_secs(options.seconds);
    let mut tasks = Vec::new();
    for _ in 0..options.streams {
        let (mut pipe, moved) = (open(command).await?, moved.clone());
        tasks.push(tokio::spawn(async move {
            if command == "down" {
                while let Ok(Some(chunk)) = pipe.recv().await {
                    moved.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            } else {
                let chunk = payload();
                while pipe.send(chunk.clone()).await.is_ok() {
                    moved.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            }
        }));
    }
    let prober = tokio::spawn(ping_loop(open("echo").await?, end));
    let _reap = Reap(
        tasks
            .iter()
            .map(|t| t.abort_handle())
            .chain([prober.abort_handle()])
            .collect(),
    );

    // The first part of a transfer is slow start and window growth; it is not counted.
    let warm = start + Duration::from_secs_f64((options.seconds as f64 * 0.25).min(1.0));
    let mut readings = vec![0u64];
    let mut at_warm = None;
    let mut tick = interval(SAMPLE_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tick.tick().await;
        let (now, bytes) = (Instant::now(), moved.load(Ordering::Relaxed));
        readings.push(bytes);
        if at_warm.is_none() && now >= warm {
            at_warm = Some((now, bytes));
        }
        let n = readings.len();
        if n > 4 && (n - 1) % 2 == 0 {
            let recent = mbps(bytes - readings[n - 5], SAMPLE_EVERY * 4);
            progress(format!("{arrow} {recent:.0} Mbit/s"));
        }
        if now >= end {
            break;
        }
    }
    for task in &tasks {
        task.abort();
    }
    let bytes = moved.load(Ordering::Relaxed);
    let (warm_at, warm_bytes) = at_warm.unwrap_or((start, 0));
    let average = mbps(bytes - warm_bytes, Instant::now().duration_since(warm_at));
    let peak = readings
        .windows(5)
        .map(|w| mbps(w[4] - w[0], SAMPLE_EVERY * 4))
        .fold(average, f64::max);
    let latency = prober.await.unwrap_or_default();
    Ok((
        Rate {
            mbps: average,
            peak_mbps: peak,
            bytes,
        },
        latency,
    ))
}

/// Sends a small numbered message every [`PING_EVERY`] until `until`, waiting for each
/// echo; the messages that never come back count as lost.
async fn ping_loop(mut pipe: Pipe, until: Instant) -> Latency {
    let (mut rtts, mut sent, mut seq) = (Vec::new(), 0u32, 0u64);
    let mut inbox = BytesMut::new();
    let mut next = Instant::now();
    'pings: while Instant::now() < until {
        sleep_until(next).await;
        next += PING_EVERY;
        let started = Instant::now();
        sent += 1;
        if pipe
            .send(Bytes::copy_from_slice(&seq.to_be_bytes()))
            .await
            .is_err()
        {
            break;
        }
        let answered = timeout(PING_TIMEOUT, async {
            loop {
                while inbox.len() >= 8 {
                    let got = u64::from_be_bytes(inbox.split_to(8)[..].try_into().unwrap());
                    // Older echoes that arrived late are skipped.
                    if got == seq {
                        return true;
                    }
                }
                match pipe.recv().await {
                    Ok(Some(chunk)) => inbox.extend_from_slice(&chunk),
                    _ => return false,
                }
            }
        })
        .await;
        match answered {
            Ok(true) => rtts.push(started.elapsed()),
            // The stream ended or failed: nothing more will come.
            Ok(false) => break 'pings,
            Err(_) => {}
        }
        seq += 1;
    }
    Latency::from_samples(sent, &rtts)
}

/// Numbered datagrams at game rate, echoed by the exit: loss and jitter.
async fn udp_test(pipe: Pipe, seconds: u64) -> Latency {
    let Some(stream) = pipe.datagrams() else {
        return Latency::default();
    };
    let count = seconds * UDP_RATE;
    let begin = Instant::now();
    let mut rtts: Vec<Option<Duration>> = vec![None; count as usize];
    let mut tick = interval(Duration::from_micros(1_000_000 / UDP_RATE));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sent = 0u64;
    let mut done = None;
    loop {
        tokio::select! {
            _ = tick.tick(), if sent < count => {
                let mut packet = vec![0u8; UDP_PACKET];
                packet[..4].copy_from_slice(&(sent as u32).to_be_bytes());
                packet[4..12].copy_from_slice(&(begin.elapsed().as_micros() as u64).to_be_bytes());
                stream.send_datagram(Bytes::from(packet));
                sent += 1;
                if sent == count {
                    done = Some(Instant::now() + UDP_GRACE);
                }
            }
            received = stream.recv_datagram() => match received {
                Ok(Some(d)) if d.len() >= 12 => {
                    let seq = u32::from_be_bytes(d[..4].try_into().unwrap()) as usize;
                    let sent_at = u64::from_be_bytes(d[4..12].try_into().unwrap());
                    if let Some(slot) = rtts.get_mut(seq).filter(|s| s.is_none()) {
                        *slot = Some(begin.elapsed().saturating_sub(Duration::from_micros(sent_at)));
                    }
                }
                Ok(Some(_)) => {}
                _ => break,
            },
            _ = async { sleep_until(done.unwrap_or_else(Instant::now)).await }, if done.is_some() => break,
        }
    }
    let answered: Vec<Duration> = rtts.into_iter().flatten().collect();
    Latency::from_samples(count as u32, &answered)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::mux::{MuxSession, SessionConfig, Side};
    use crate::proto::Open;

    fn config() -> SessionConfig {
        SessionConfig {
            stream_window: 256 * 1024,
            max_streams: 64,
            keepalive: Duration::from_secs(30),
            coalesce: true,
            datagram_buffer: 256 * 1024,
            datagram_queue: 128,
        }
    }

    /// An entry and an exit over an in-memory link, the exit serving test streams.
    pub(crate) fn tunnel() -> Arc<MuxSession> {
        let (a, b) = tokio::io::duplex(1 << 20);
        let client = Arc::new(MuxSession::new(a, Side::Client, config()));
        let server = MuxSession::new(b, Side::Server, config());
        tokio::spawn(async move {
            while let Some((stream, syn)) = server.accept().await {
                let open = Open::decode(&syn).unwrap();
                tokio::spawn(async move {
                    let pipe = Pipe::Stream(SessionStream::Kmux(stream));
                    let _ = serve(pipe, &open.target).await;
                });
            }
        });
        client
    }

    fn opener(
        client: Arc<MuxSession>,
    ) -> impl Fn(&'static str) -> std::future::Ready<io::Result<Pipe>> {
        move |command| {
            let mut syn = Vec::new();
            Open::speedtest(command).encode(&mut syn);
            std::future::ready(
                client
                    .open(Bytes::from(syn))
                    .map(|s| Pipe::Stream(SessionStream::Kmux(s))),
            )
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_whole_test_measures_speed_latency_and_udp() {
        let client = tunnel();
        let options = Options {
            seconds: 2,
            streams: 2,
            udp: true,
        };
        let mut lines = Vec::new();
        let report = run(opener(client), options, |l| lines.push(l))
            .await
            .unwrap();
        assert!(report.download.mbps > 1.0, "{report:?}");
        assert!(report.upload.mbps > 1.0, "{report:?}");
        assert!(report.download.peak_mbps >= report.download.mbps * 0.9);
        assert!(report.download.bytes > 0 && report.upload.bytes > 0);
        // A message every 100 ms for 2 s, all answered on an in-memory link.
        assert!(
            report.idle.received >= 10 && report.idle.loss_percent() < 10.0,
            "{report:?}"
        );
        assert!(report.download_latency.received > 5, "{report:?}");
        assert!(report.upload_latency.received > 5, "{report:?}");
        let udp = report.udp.clone().expect("udp was tested");
        assert_eq!(udp.sent, 128);
        assert!(udp.received >= 120, "{udp:?}");
        assert!(udp.p50_ms < 500.0);
        assert!(lines.iter().any(|l| l.contains("Mbit/s")), "{lines:?}");
        // Each phase tells its result as it ends, for the panel to show at once.
        for phase in ["idle", "download", "upload", "udp"] {
            let prefix = format!("{RESULT_LINE}{phase} {{");
            assert!(
                lines.iter().any(|l| l.starts_with(&prefix)),
                "{phase}: {lines:?}"
            );
        }
        let download = lines.iter().find(|l| l.starts_with("@download ")).unwrap();
        let rate: Rate = serde_json::from_str(&download["@download ".len()..]).unwrap();
        assert_eq!(rate, report.download);
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        // The report survives the trip the control socket gives it.
        let text = toml::to_string(&report).unwrap();
        assert_eq!(toml::from_str::<Report>(&text).unwrap(), report);
    }

    #[tokio::test]
    async fn a_test_that_is_cut_off_leaves_no_task_behind() {
        let task = tokio::spawn(std::future::pending::<()>());
        let reap = Reap(vec![task.abort_handle()]);
        drop(reap);
        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn a_silent_peer_is_reported() {
        // Streams nobody serves: no echo ever comes back.
        let (a, b) = tokio::io::duplex(1 << 16);
        let client = Arc::new(MuxSession::new(a, Side::Client, config()));
        let server = MuxSession::new(b, Side::Server, config());
        tokio::spawn(async move {
            while let Some((_stream, _syn)) = server.accept().await {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        let options = Options {
            seconds: 1,
            streams: 1,
            udp: false,
        };
        let err = run(opener(client), options, |_| {}).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{err}");
        assert!(err.to_string().contains("older than v0.6"), "{err}");
    }

    #[test]
    fn settings_are_bounded() {
        let ok = Options::default();
        assert!(ok.validate().is_ok());
        for bad in [
            Options { seconds: 0, ..ok },
            Options {
                seconds: MAX_SECONDS + 1,
                ..ok
            },
            Options { streams: 0, ..ok },
            Options {
                streams: MAX_STREAMS + 1,
                ..ok
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn latency_statistics() {
        let ms = Duration::from_millis;
        let l = Latency::from_samples(5, &[ms(10), ms(20), ms(10), ms(30)]);
        assert_eq!((l.sent, l.received), (5, 4));
        assert_eq!(l.p50_ms, 20.0);
        assert_eq!(l.p99_ms, 30.0);
        // |20-10|, |10-20|, |30-10| over 3 gaps.
        assert_eq!(l.jitter_ms, round1(40.0 / 3.0));
        assert_eq!(l.loss_percent(), 20.0);
        assert_eq!(Latency::from_samples(0, &[]), Latency::default());
    }
}
