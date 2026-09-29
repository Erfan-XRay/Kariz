//! The control socket: a local socket on the entry side through which tools use the
//! running daemon's live sessions. Today that is `kariz speedtest`.
//!
//! Why a socket and not a second process that dials the exit itself: in reverse mode the
//! exit dials the entry, and the daemon owns the port it dials; and a test through the
//! daemon's own sessions measures exactly what users' traffic gets.
//!
//! Wire format, one request per connection:
//!
//! ```text
//! client -> daemon   speedtest seconds=10 streams=4 udp=1 \n
//! daemon -> client   # progress text \n        (any number)
//!                    ---\n <the report, TOML>   or   ! error text \n
//! ```
//!
//! The socket is a Unix socket (Linux servers) that only its owner can open; on other
//! systems there is no control socket.

use std::future::Future;
use std::io;

use bytes::Bytes;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use crate::channel::Channel;
use crate::proto::Open;
use crate::speedtest::{self, Options, Pipe, Report};

/// Longest request line the daemon reads.
const MAX_REQUEST: u64 = 1024;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// `speedtest seconds=10 streams=4 udp=1`
pub fn format_request(options: &Options) -> String {
    format!(
        "speedtest seconds={} streams={} udp={}\n",
        options.seconds,
        options.streams,
        u8::from(options.udp)
    )
}

fn parse_request(line: &str) -> io::Result<Options> {
    let mut words = line.split_whitespace();
    if words.next() != Some("speedtest") {
        return Err(invalid(
            "unknown request (this daemon only knows `speedtest`)",
        ));
    }
    let mut options = Options::default();
    for word in words {
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| invalid(format!("bad setting {word:?}")))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| invalid(format!("{key} needs a number")))
        };
        match key {
            "seconds" => options.seconds = number()?,
            "streams" => options.streams = number()? as usize,
            "udp" => options.udp = number()? != 0,
            _ => return Err(invalid(format!("unknown setting {key:?}"))),
        }
    }
    options.validate()?;
    Ok(options)
}

/// Serves one connection: reads the request, runs the test through `open` (which opens
/// a channel to the exit side for an encoded open request) and writes progress and the
/// report back. One test at a time (`busy`): a test uses the tunnel's bandwidth.
pub async fn handle<S, O, F>(stream: S, open: &O, busy: &Mutex<()>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    O: Fn(Bytes) -> F,
    F: Future<Output = io::Result<Channel>> + Send,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut line = String::new();
    BufReader::new(reader.take(MAX_REQUEST))
        .read_line(&mut line)
        .await?;
    let options = match parse_request(&line) {
        Ok(options) => options,
        Err(e) => return reply_error(&mut writer, &e.to_string()).await,
    };
    let Ok(_running) = busy.try_lock() else {
        return reply_error(&mut writer, "another speed test is already running").await;
    };

    let opener = |command: &'static str| {
        let mut syn = Vec::new();
        Open::speedtest(command).encode(&mut syn);
        let opened = open(Bytes::from(syn));
        async move { opened.await.map(Pipe::from) }
    };
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<String>();
    let test = speedtest::run(opener, options, move |line| {
        let _ = progress_tx.send(line);
    });
    tokio::pin!(test);
    // A client that goes away ends the test: the write fails and `test` is dropped.
    let result = loop {
        tokio::select! {
            Some(line) = progress_rx.recv() => {
                writer.write_all(format!("# {line}\n").as_bytes()).await?;
            }
            result = &mut test => break result,
        }
    };
    while let Ok(line) = progress_rx.try_recv() {
        writer.write_all(format!("# {line}\n").as_bytes()).await?;
    }
    match result {
        Ok(report) => {
            let doc = toml::to_string(&report).map_err(io::Error::other)?;
            writer.write_all(format!("---\n{doc}").as_bytes()).await?;
            writer.flush().await
        }
        Err(e) => reply_error(&mut writer, &e.to_string()).await,
    }
}

async fn reply_error<W: AsyncWrite + Unpin>(writer: &mut W, text: &str) -> io::Result<()> {
    writer
        .write_all(format!("! {}\n", text.replace('\n', " ")).as_bytes())
        .await?;
    writer.flush().await
}

/// Asks a daemon for a speed test over `stream`; `on_progress` gets each progress line.
pub async fn request<S>(
    stream: S,
    options: Options,
    mut on_progress: impl FnMut(&str),
) -> io::Result<Report>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(format_request(&options).as_bytes())
        .await?;
    writer.flush().await?;
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(progress) = line.strip_prefix("# ") {
            on_progress(progress);
        } else if let Some(error) = line.strip_prefix("! ") {
            return Err(io::Error::other(error.to_string()));
        } else if line == "---" {
            let mut doc = String::new();
            while let Some(rest) = lines.next_line().await? {
                doc.push_str(&rest);
                doc.push('\n');
            }
            return toml::from_str(&doc)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the daemon closed the connection before the test finished",
    ))
}

#[cfg(unix)]
pub use unix::{connect, serve};

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use bytes::Bytes;
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::Mutex;
    use tracing::{info, warn};

    use super::{handle, Channel, Future};

    /// Connects to a daemon's control socket.
    pub async fn connect(path: &Path) -> io::Result<UnixStream> {
        UnixStream::connect(path).await.map_err(|e| {
            let hint = match e.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    ": is this tunnel running? (the entry side has to be)"
                }
                io::ErrorKind::PermissionDenied => ": run it as the user that runs Kariz",
                _ => "",
            };
            io::Error::new(
                e.kind(),
                format!(
                    "cannot reach the control socket {}: {e}{hint}",
                    path.display()
                ),
            )
        })
    }

    /// Runs the control socket for the daemon's whole life. It never returns: a problem
    /// with it (an unwritable directory) is logged, and the tunnel goes on without it.
    pub async fn serve<O, F>(path: PathBuf, open: O) -> anyhow::Result<()>
    where
        O: Fn(Bytes) -> F + Send + Sync + 'static,
        F: Future<Output = io::Result<Channel>> + Send + 'static,
    {
        if let Err(e) = listen(&path, open).await {
            warn!(socket = %path.display(), error = %e, "no control socket (kariz speedtest will not work)");
        }
        std::future::pending::<()>().await;
        Ok(())
    }

    async fn listen<O, F>(path: &Path, open: O) -> io::Result<()>
    where
        O: Fn(Bytes) -> F + Send + Sync + 'static,
        F: Future<Output = io::Result<Channel>> + Send + 'static,
    {
        if UnixStream::connect(path).await.is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another Kariz is answering on this socket",
            ));
        }
        // Made private before it gets its name, so it is never open to others.
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".new");
        let temporary = PathBuf::from(temporary);
        let _ = std::fs::remove_file(&temporary);
        let listener = UnixListener::bind(&temporary)?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&temporary, path)?;
        info!(socket = %path.display(), "control socket ready");

        let open = Arc::new(open);
        let busy = Arc::new(Mutex::new(()));
        loop {
            let (connection, _) = listener.accept().await?;
            let (open, busy) = (open.clone(), busy.clone());
            tokio::spawn(async move {
                let _ = handle(connection, &*open, &busy).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::session::SessionStream;
    use crate::speedtest::tests::tunnel;

    /// Opens channels on the in-memory tunnel of the speed test's tests.
    fn opener() -> impl Fn(Bytes) -> std::future::Ready<io::Result<Channel>> + Send + Sync {
        let client = tunnel();
        move |syn| {
            std::future::ready(
                client
                    .open(syn)
                    .map(|s| Channel::Stream(SessionStream::Kmux(s))),
            )
        }
    }

    #[test]
    fn requests_round_trip_and_bad_ones_are_refused() {
        let options = Options {
            seconds: 7,
            streams: 3,
            udp: false,
        };
        assert_eq!(parse_request(&format_request(&options)).unwrap(), options);
        assert_eq!(parse_request("speedtest\n").unwrap(), Options::default());
        for bad in [
            "",
            "rm -rf /",
            "speedtest seconds",
            "speedtest seconds=x",
            "speedtest seconds=0",
            "speedtest streams=99",
            "speedtest color=red",
        ] {
            assert!(parse_request(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_client_gets_progress_and_the_report() {
        let (client_end, daemon_end) = tokio::io::duplex(1 << 16);
        let (open, busy) = (opener(), Mutex::new(()));
        let daemon = tokio::spawn(async move { handle(daemon_end, &open, &busy).await });
        let options = Options {
            seconds: 1,
            streams: 1,
            udp: true,
        };
        let mut lines = Vec::new();
        let report = request(client_end, options, |l| lines.push(l.to_string()))
            .await
            .unwrap();
        daemon.await.unwrap().unwrap();
        assert!(
            report.download.mbps > 0.0 && report.upload.mbps > 0.0,
            "{report:?}"
        );
        assert!(report.udp.is_some());
        assert!(lines.iter().any(|l| l.starts_with("latency")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("Mbit/s")), "{lines:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn only_one_test_runs_at_a_time_and_bad_requests_get_an_error() {
        let open = Arc::new(opener());
        let busy = Arc::new(Mutex::new(()));
        let start = |line: &'static str| {
            let (mut client, daemon_end) = tokio::io::duplex(1 << 16);
            let (open, busy) = (open.clone(), busy.clone());
            tokio::spawn(async move { handle(daemon_end, &*open, &busy).await });
            async move {
                client.write_all(line.as_bytes()).await.unwrap();
                let mut answer = String::new();
                client.read_to_string(&mut answer).await.unwrap();
                answer
            }
        };
        let first = tokio::spawn(start("speedtest seconds=1 streams=1 udp=0\n"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        let second = start("speedtest seconds=1\n").await;
        assert!(second.starts_with("! another speed test"), "{second}");
        let first = first.await.unwrap();
        assert!(first.contains("---\n"), "{first}");
        let bad = start("speedtest seconds=0\n").await;
        assert!(bad.starts_with("! seconds must be"), "{bad}");
        let unknown = start("shutdown\n").await;
        assert!(unknown.starts_with("! unknown request"), "{unknown}");
    }
}
