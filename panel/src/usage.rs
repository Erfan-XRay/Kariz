//! How much the servers and tunnels have used: the volume that went over the network, as
//! a server's provider counts it.
//!
//! A **subject** is a server's network cards (`srv:ID`, from `Health::net`) or one side of
//! a tunnel (`tun:NAME@ID`, from the daemon's `wire` counters). Each reading of a subject
//! is compared with the one before (the **cursor**, kept in the database, so a panel that
//! restarts or was away loses nothing and counts nothing twice); a counter that started
//! again (a restarted daemon, a rebooted server) counts from zero. What a side moved is
//! split into:
//!
//! * `data`: the users' bytes, on each leg they cross (users and entry, the link, exit and
//!   targets; what goes over the loopback reaches no network card and is left out);
//! * `proto`: what the tunnel adds on the link: encryption, framing, handshakes, pings,
//!   UDP copies;
//! * `resend`: sent again: TCP retransmissions, KCP resends, FEC parity;
//! * `headers`: IP, TCP and UDP headers, the link layer's (Ethernet) and a GRE or VXLAN
//!   link's own.
//!
//! `rx` and `tx` are the same bytes by direction. A daemon older than the `wire` counters
//! gives the users' bytes alone; those sides are estimated (`est`).
//!
//! Sums go to SQLite every half minute: 15-minute buckets kept 90 days, and days of the
//! panel's time zone kept for good.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use anyhow::Result;
use kariz::stats::Status;
use kariz::wire::DirStatus;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::now;
use crate::calendar::Calendar;
use crate::db::Db;
use crate::wire::NetTotals;

/// Seconds in a bucket.
pub const QUARTER: i64 = 900;
/// How long buckets are kept.
pub const KEEP_QUARTERS: i64 = 90 * 86_400;
/// A counter whose start moved by more than this started again.
const RESTART_SLACK: i64 = 30;
/// The panel's time zone, minutes east of UTC, when none was chosen: Tehran.
pub const DEFAULT_OFFSET: i64 = 210;
const META_OFFSET: &str = "usage_offset";
const META_CALENDAR: &str = "usage_calendar";

/// What a subject moved in some time, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub rx: u64,
    pub tx: u64,
    pub data: u64,
    pub proto: u64,
    pub resend: u64,
    pub headers: u64,
    /// The users' own bytes, each way (once, not per leg).
    pub up: u64,
    pub down: u64,
    /// Of `rx + tx`, what is estimated (a daemon without the wire counters).
    pub est: u64,
}

impl Row {
    fn add(&mut self, o: &Row) {
        self.rx += o.rx;
        self.tx += o.tx;
        self.data += o.data;
        self.proto += o.proto;
        self.resend += o.resend;
        self.headers += o.headers;
        self.up += o.up;
        self.down += o.down;
        self.est += o.est;
    }

    fn is_empty(&self) -> bool {
        *self == Row::default()
    }

    fn from_sql(r: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Self> {
        let g = |i: usize| -> rusqlite::Result<u64> {
            Ok(r.get::<_, Option<i64>>(first + i)?.unwrap_or(0).max(0) as u64)
        };
        Ok(Self {
            rx: g(0)?,
            tx: g(1)?,
            data: g(2)?,
            proto: g(3)?,
            resend: g(4)?,
            headers: g(5)?,
            up: g(6)?,
            down: g(7)?,
            est: g(8)?,
        })
    }
}

const COLUMNS: &str = "rx, tx, data, proto, resend, headers, up, down, est";
const SUMS: &str = "sum(rx), sum(tx), sum(data), sum(proto), sum(resend), sum(headers), \
                    sum(up), sum(down), sum(est)";

/// Which subjects a question is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Of {
    /// A server's network cards.
    Server(String),
    /// Every tunnel side on a server.
    ServerTunnels(String),
    /// Both sides of a tunnel.
    Tunnel(String),
}

impl Of {
    pub fn parse(kind: &str, id: &str) -> Option<Self> {
        if id.is_empty() || id.len() > 96 {
            return None;
        }
        Some(match kind {
            "server" => Self::Server(id.to_owned()),
            "server-tunnels" => Self::ServerTunnels(id.to_owned()),
            "tunnel" => Self::Tunnel(id.to_owned()),
            _ => return None,
        })
    }

    fn matches(&self, subject: &str) -> bool {
        match self {
            Self::Server(id) => subject.strip_prefix("srv:") == Some(id),
            Self::ServerTunnels(id) => side_of(subject).is_some_and(|(_, s)| s == id),
            Self::Tunnel(name) => side_of(subject).is_some_and(|(n, _)| n == name),
        }
    }

    /// A condition on `subject` and its one parameter.
    fn sql(&self) -> (&'static str, String) {
        match self {
            Self::Server(id) => ("subject = ?1", format!("srv:{id}")),
            // Tunnel names and server ids have no `@`, so a suffix and a prefix say it.
            Self::ServerTunnels(id) => (
                "subject LIKE 'tun:%' AND substr(subject, -length(?1)) = ?1",
                format!("@{id}"),
            ),
            Self::Tunnel(name) => (
                "substr(subject, 1, length(?1)) = ?1",
                format!("tun:{name}@"),
            ),
        }
    }

    /// The key its "counted since" is kept under.
    fn since_key(&self) -> Option<String> {
        match self {
            Self::Server(id) => Some(format!("srv:{id}")),
            Self::Tunnel(name) => Some(format!("tun:{name}")),
            Self::ServerTunnels(_) => None,
        }
    }
}

fn side_key(name: &str, server: &str) -> String {
    format!("tun:{name}@{server}")
}

/// A tunnel side's name and server.
fn side_of(subject: &str) -> Option<(&str, &str)> {
    subject.strip_prefix("tun:")?.split_once('@')
}

/// The key a subject's "counted since" is kept under: the server, or the whole tunnel.
fn since_key_of(subject: &str) -> String {
    match side_of(subject) {
        Some((name, _)) => format!("tun:{name}"),
        None => subject.to_owned(),
    }
}

/// The last reading of a subject's counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Cursor {
    /// When the counters started (the daemon, or the server's boot), unix seconds.
    start: i64,
    at: i64,
    raw: Vec<u64>,
}

/// What changed since the last reading: `None` for the first one (nothing to compare).
fn delta(before: Option<&Cursor>, start: i64, raw: &[u64]) -> Option<Vec<u64>> {
    let before = before?;
    if start > before.start + RESTART_SLACK {
        // Started again after the last reading: everything since is new.
        return Some(raw.to_vec());
    }
    if before.raw.len() != raw.len() {
        return None;
    }
    // A counter that went back without a restart (an interface made again) gives
    // nothing this time and counts on from where it is.
    Some(
        raw.iter()
            .zip(&before.raw)
            .map(|(now, was)| now.saturating_sub(*was))
            .collect(),
    )
}

/// A tunnel side's counters, flat: the users' bytes up and down, speed tests, then
/// bytes, packets, headers and resends sent and received on each leg.
fn side_raw(status: &Status) -> Vec<u64> {
    let mut raw = vec![status.totals.bytes_up, status.totals.bytes_down];
    if let Some(w) = &status.wire {
        raw.push(w.tests);
        for leg in [&w.user, &w.link, &w.target] {
            for d in [&leg.sent, &leg.received] {
                raw.extend([d.bytes, d.packets, d.headers, d.resend]);
            }
        }
    }
    raw
}

/// Turns a side's change (`side_raw` layout) into usage. `link_header`: bytes of
/// link-layer header per packet on the server; `encap`: what a GRE or VXLAN link adds to
/// each packet of the tunnel's own link (0 when it goes over the internet).
fn side_row(d: &[u64], link_header: u64, encap: u64) -> Row {
    let (up, down) = (d[0], d[1]);
    if d.len() < 3 + 24 {
        // No wire counters: each user byte comes in once and goes out once.
        let both = up + down;
        return Row {
            rx: both,
            tx: both,
            data: 2 * both,
            up,
            down,
            est: 2 * both,
            ..Row::default()
        };
    }
    let tests = d[2];
    let mut row = Row {
        up,
        down,
        ..Row::default()
    };
    let (mut edge_payload, mut link_payload) = (0, 0);
    for leg in 0..3 {
        for dir in 0..2 {
            let at = 3 + leg * 8 + dir * 4;
            let s = DirStatus {
                bytes: d[at],
                packets: d[at + 1],
                headers: d[at + 2],
                resend: d[at + 3],
            };
            let extra = s.packets * (link_header + if leg == 1 { encap } else { 0 });
            let total = s.bytes + extra;
            if dir == 0 {
                row.tx += total;
            } else {
                row.rx += total;
            }
            row.headers += s.headers + extra;
            row.resend += s.resend;
            if leg == 1 {
                link_payload += s.payload();
            } else {
                edge_payload += s.payload();
            }
        }
    }
    // The link carries the users' bytes and speed tests; the rest of it is the tunnel's.
    let carried = link_payload.min(up + down + tests);
    row.data = edge_payload + carried;
    row.proto = link_payload - carried;
    row
}

fn server_row(d: &[u64]) -> Row {
    Row {
        rx: d[0],
        tx: d[1],
        ..Row::default()
    }
}

/// The monthly quota of a server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub limit: u64,
    /// What counts: `both` directions, only `tx` (sent, as some providers bill) or `rx`.
    pub count: String,
    /// The day of the month it renews on (1-31; a shorter month uses its last day).
    pub reset_day: u32,
    /// Percent at which a warning goes out (the second goes at 100).
    pub warn: u32,
}

impl Quota {
    pub fn valid(&self) -> bool {
        self.limit > 0
            && matches!(self.count.as_str(), "both" | "tx" | "rx")
            && (1..=31).contains(&self.reset_day)
            && (1..=99).contains(&self.warn)
    }

    fn used(&self, row: &Row) -> u64 {
        match self.count.as_str() {
            "tx" => row.tx,
            "rx" => row.rx,
            _ => row.rx + row.tx,
        }
    }
}

/// A quota with where it stands now.
#[derive(Debug, Clone, Serialize)]
pub struct QuotaState {
    #[serde(flatten)]
    pub quota: Quota,
    pub used: u64,
    pub period_start: i64,
    pub period_end: i64,
}

/// A quota crossing its warning or its limit.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaCrossed {
    pub server: String,
    pub used: u64,
    pub limit: u64,
    /// At the limit (else at the warning).
    pub full: bool,
}

/// How usage is shown: the calendar and the time zone (minutes east of UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub calendar: Calendar,
    pub offset: i64,
}

impl Settings {
    pub fn valid(&self) -> bool {
        (-720..=840).contains(&self.offset) && self.offset % 15 == 0
    }
}

#[derive(Default)]
struct State {
    loaded: bool,
    cursors: HashMap<String, Cursor>,
    dirty: HashSet<String>,
    /// Sums not written yet, by subject and bucket.
    pending: HashMap<(String, i64), Row>,
    pruned: i64,
    /// What a private-network link adds to each packet of a tunnel's link, by tunnel, as
    /// its dialing side told.
    encaps: HashMap<String, u32>,
}

pub struct Usage {
    db: Db,
    state: Mutex<State>,
}

impl Usage {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.loaded {
            s.loaded = true;
            if let Ok(cursors) = self.load_cursors() {
                s.cursors = cursors;
            }
        }
        s
    }

    fn load_cursors(&self) -> Result<HashMap<String, Cursor>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT subject, start, at, raw FROM usage_cursors")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = HashMap::new();
        for (subject, start, at, raw) in rows.flatten() {
            if let Ok(raw) = serde_json::from_str(&raw) {
                out.insert(subject, Cursor { start, at, raw });
            }
        }
        Ok(out)
    }

    pub fn settings(&self) -> Settings {
        let meta = |k| self.db.meta(k).ok().flatten();
        Settings {
            calendar: meta(META_CALENDAR)
                .and_then(|c| Calendar::parse(&c))
                .unwrap_or_default(),
            offset: meta(META_OFFSET)
                .and_then(|o| o.parse().ok())
                .unwrap_or(DEFAULT_OFFSET),
        }
    }

    /// Changes how usage is shown. Days already kept stay as they were cut.
    pub fn set_settings(&self, s: Settings) -> Result<()> {
        self.flush()?;
        self.db.set_meta(META_CALENDAR, s.calendar.name())?;
        self.db.set_meta(META_OFFSET, &s.offset.to_string())
    }

    fn take(
        &self,
        s: &mut State,
        subject: String,
        at: i64,
        start: i64,
        raw: Vec<u64>,
    ) -> Option<Vec<u64>> {
        let d = delta(s.cursors.get(&subject), start, &raw);
        s.cursors.insert(subject.clone(), Cursor { start, at, raw });
        s.dirty.insert(subject);
        d
    }

    fn add(s: &mut State, subject: &str, at: i64, row: Row) {
        if row.is_empty() {
            return;
        }
        let bucket = at - at.rem_euclid(QUARTER);
        s.pending
            .entry((subject.to_owned(), bucket))
            .or_default()
            .add(&row);
    }

    /// A reading of a server's network cards. `uptime`: the server's, to tell a reboot.
    pub fn record_server(&self, server: &str, at: i64, uptime: Option<u64>, net: &NetTotals) {
        let subject = format!("srv:{server}");
        let start = at - uptime.unwrap_or(0) as i64;
        let raw = vec![net.rx_bytes, net.tx_bytes, net.rx_packets, net.tx_packets];
        let mut s = self.state();
        if let Some(d) = self.take(&mut s, subject.clone(), at, start, raw) {
            Self::add(&mut s, &subject, at, server_row(&d));
        }
    }

    /// A reading of one side of a tunnel on `server`. `encap`: what a private-network link
    /// adds to each packet of the tunnel's link, when this side can tell (else what the
    /// other side told).
    pub fn record_side(
        &self,
        name: &str,
        server: &str,
        at: i64,
        status: &Status,
        link_header: u32,
        encap: Option<u32>,
    ) {
        let subject = side_key(name, server);
        let start = at - status.uptime_secs as i64;
        let raw = side_raw(status);
        let mut s = self.state();
        let encap = match encap {
            Some(e) => {
                s.encaps.insert(name.to_owned(), e);
                e
            }
            None => s.encaps.get(name).copied().unwrap_or(0),
        };
        if let Some(d) = self.take(&mut s, subject.clone(), at, start, raw) {
            let row = side_row(&d, link_header.into(), encap.into());
            Self::add(&mut s, &subject, at, row);
        }
    }

    /// Writes what is pending, and drops buckets older than [`KEEP_QUARTERS`] once an hour.
    pub fn flush(&self) -> Result<()> {
        let offset = self.settings().offset * 60;
        let (pending, cursors) = {
            let mut s = self.state();
            let pending = std::mem::take(&mut s.pending);
            let dirty = std::mem::take(&mut s.dirty);
            let cursors: Vec<(String, Cursor)> = dirty
                .into_iter()
                .filter_map(|k| s.cursors.get(&k).map(|c| (k, c.clone())))
                .collect();
            (pending, cursors)
        };
        let at = now();
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        for ((subject, ts), r) in &pending {
            let day = (ts + offset).div_euclid(86_400) * 86_400 - offset;
            for (table, key, value) in [("usage", "ts", *ts), ("usage_daily", "day", day)] {
                tx.execute(
                    &format!(
                        "INSERT INTO {table} (subject, {key}, {COLUMNS})
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                         ON CONFLICT (subject, {key}) DO UPDATE SET
                           rx = rx + excluded.rx, tx = tx + excluded.tx,
                           data = data + excluded.data, proto = proto + excluded.proto,
                           resend = resend + excluded.resend,
                           headers = headers + excluded.headers,
                           up = up + excluded.up, down = down + excluded.down,
                           est = est + excluded.est"
                    ),
                    params![
                        subject,
                        value,
                        r.rx as i64,
                        r.tx as i64,
                        r.data as i64,
                        r.proto as i64,
                        r.resend as i64,
                        r.headers as i64,
                        r.up as i64,
                        r.down as i64,
                        r.est as i64
                    ],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO usage_since (subject, since) VALUES (?1, ?2)",
                params![since_key_of(subject), (*ts).min(at)],
            )?;
        }
        for (subject, c) in &cursors {
            tx.execute(
                "INSERT INTO usage_cursors (subject, start, at, raw) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (subject) DO UPDATE SET
                   start = excluded.start, at = excluded.at, raw = excluded.raw",
                params![subject, c.start, c.at, serde_json::to_string(&c.raw)?],
            )?;
        }
        tx.commit()?;
        drop(conn);
        let mut s = self.state();
        if at - s.pruned >= 3600 {
            s.pruned = at;
            drop(s);
            self.db
                .conn()
                .execute("DELETE FROM usage WHERE ts < ?1", [at - KEEP_QUARTERS])?;
        }
        Ok(())
    }

    /// Writes pending sums every 30 seconds, for as long as the panel runs.
    pub async fn run(self: std::sync::Arc<Self>) {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            if let Err(e) = self.flush() {
                tracing::warn!(error = %e, "could not save usage");
            }
        }
    }

    fn pending_of(&self, of: &Of) -> Vec<(i64, Row)> {
        let s = self.state();
        s.pending
            .iter()
            .filter(|((subject, _), _)| of.matches(subject))
            .map(|((_, ts), r)| (*ts, *r))
            .collect()
    }

    /// Usage over `from..to` (unix seconds), in buckets (`daily`: days of the panel's
    /// time zone; else 15 minutes), oldest first. Includes what is not written yet.
    pub fn series(&self, of: &Of, from: i64, to: i64, daily: bool) -> Result<Vec<(i64, Row)>> {
        let (cond, param) = of.sql();
        let (table, key) = if daily {
            ("usage_daily", "day")
        } else {
            ("usage", "ts")
        };
        let mut sums: std::collections::BTreeMap<i64, Row> = std::collections::BTreeMap::new();
        {
            let conn = self.db.conn();
            let sql = format!(
                "SELECT {key}, {SUMS} FROM {table}
                 WHERE {cond} AND {key} >= ?2 AND {key} < ?3 GROUP BY {key}",
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![param, from, to], |r| {
                Ok((r.get::<_, i64>(0)?, Row::from_sql(r, 1)?))
            })?;
            for row in rows {
                let (k, r) = row?;
                sums.entry(k).or_default().add(&r);
            }
        }
        let offset = self.settings().offset * 60;
        for (ts, r) in self.pending_of(of) {
            let k = if daily {
                (ts + offset).div_euclid(86_400) * 86_400 - offset
            } else {
                ts
            };
            if (from..to).contains(&k) {
                sums.entry(k).or_default().add(&r);
            }
        }
        Ok(sums.into_iter().collect())
    }

    /// Everything a subject moved since it was counted, and since each of `marks` (unix
    /// seconds; exact to the bucket within 90 days, to the day before that).
    pub fn totals(&self, of: &Of, marks: &[i64]) -> Result<Totals> {
        let all = self.sum(of, None)?;
        let marks = marks
            .iter()
            .map(|&m| self.sum(of, Some(m)))
            .collect::<Result<_>>()?;
        let since = match of.since_key() {
            Some(key) => self
                .db
                .conn()
                .query_row(
                    "SELECT since FROM usage_since WHERE subject = ?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()?,
            None => None,
        };
        Ok(Totals { since, all, marks })
    }

    fn sum(&self, of: &Of, from: Option<i64>) -> Result<Row> {
        let (cond, param) = of.sql();
        let mut total = {
            let conn = self.db.conn();
            match from {
                None => conn.query_row(
                    &format!("SELECT {SUMS} FROM usage_daily WHERE {cond}"),
                    params![param],
                    |r| Row::from_sql(r, 0),
                )?,
                Some(from) if from >= now() - KEEP_QUARTERS => conn.query_row(
                    &format!("SELECT {SUMS} FROM usage WHERE {cond} AND ts >= ?2"),
                    params![param, from],
                    |r| Row::from_sql(r, 0),
                )?,
                Some(from) => conn.query_row(
                    &format!("SELECT {SUMS} FROM usage_daily WHERE {cond} AND day >= ?2"),
                    params![param, from],
                    |r| Row::from_sql(r, 0),
                )?,
            }
        };
        for (ts, r) in self.pending_of(of) {
            if from.map_or(true, |f| ts >= f) {
                total.add(&r);
            }
        }
        Ok(total)
    }

    /// The tunnels with a side on `server`.
    pub fn tunnels_on(&self, server: &str) -> Result<Vec<String>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT subject FROM usage_daily
             WHERE subject LIKE 'tun:%' AND substr(subject, -length(?1)) = ?1",
        )?;
        let suffix = format!("@{server}");
        let rows = stmt.query_map([&suffix], |r| r.get::<_, String>(0))?;
        let mut names: Vec<String> = rows
            .flatten()
            .filter_map(|s| side_of(&s).map(|(n, _)| n.to_owned()))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Every subject's sums since `from` (all time with `None`), by subject.
    pub fn by_subject(&self, from: Option<i64>) -> Result<HashMap<String, Row>> {
        let mut out: HashMap<String, Row> = HashMap::new();
        {
            let conn = self.db.conn();
            let (sql, from) = match from {
                None => (
                    format!(
                        "SELECT subject, {SUMS} FROM usage_daily WHERE day >= ?1 GROUP BY subject"
                    ),
                    i64::MIN,
                ),
                Some(f) if f >= now() - KEEP_QUARTERS => (
                    format!("SELECT subject, {SUMS} FROM usage WHERE ts >= ?1 GROUP BY subject"),
                    f,
                ),
                Some(f) => (
                    format!(
                        "SELECT subject, {SUMS} FROM usage_daily WHERE day >= ?1 GROUP BY subject"
                    ),
                    f,
                ),
            };
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([from], |r| {
                Ok((r.get::<_, String>(0)?, Row::from_sql(r, 1)?))
            })?;
            for row in rows {
                let (k, r) = row?;
                out.entry(k).or_default().add(&r);
            }
        }
        let s = self.state();
        for ((subject, ts), r) in &s.pending {
            if from.map_or(true, |f| *ts >= f) {
                out.entry(subject.clone()).or_default().add(r);
            }
        }
        Ok(out)
    }

    /// When each server and tunnel has been counted from (`srv:ID`, `tun:NAME`).
    pub fn since_all(&self) -> Result<HashMap<String, i64>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT subject, since FROM usage_since")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.flatten().collect())
    }

    fn delete_where(&self, of: &Of) -> Result<()> {
        let (cond, param) = of.sql();
        {
            let mut s = self.state();
            s.pending.retain(|(subject, _), _| !of.matches(subject));
        }
        let conn = self.db.conn();
        conn.execute(&format!("DELETE FROM usage WHERE {cond}"), [&param])?;
        conn.execute(&format!("DELETE FROM usage_daily WHERE {cond}"), [&param])?;
        Ok(())
    }

    /// Starts counting a server's network cards again from now (and, with `tunnels`,
    /// each tunnel that has a side on it, both sides).
    pub fn reset_server(&self, server: &str, tunnels: bool) -> Result<()> {
        if tunnels {
            for name in self.tunnels_on(server)? {
                self.reset_tunnel(&name)?;
            }
        }
        self.delete_where(&Of::Server(server.to_owned()))?;
        self.set_since(&format!("srv:{server}"), now())
    }

    /// Starts counting a tunnel again from now.
    pub fn reset_tunnel(&self, name: &str) -> Result<()> {
        self.delete_where(&Of::Tunnel(name.to_owned()))?;
        self.set_since(&format!("tun:{name}"), now())
    }

    fn set_since(&self, key: &str, at: i64) -> Result<()> {
        self.db.conn().execute(
            "INSERT INTO usage_since (subject, since) VALUES (?1, ?2)
             ON CONFLICT (subject) DO UPDATE SET since = excluded.since",
            params![key, at],
        )?;
        Ok(())
    }

    /// A deleted tunnel: its usage goes with it.
    pub fn forget_tunnel(&self, name: &str) -> Result<()> {
        self.delete_where(&Of::Tunnel(name.to_owned()))?;
        let prefix = format!("tun:{name}@");
        self.state().cursors.retain(|k, _| !k.starts_with(&prefix));
        let conn = self.db.conn();
        conn.execute(
            "DELETE FROM usage_cursors WHERE substr(subject, 1, length(?1)) = ?1",
            [&prefix],
        )?;
        conn.execute(
            "DELETE FROM usage_since WHERE subject = ?1",
            [format!("tun:{name}")],
        )?;
        Ok(())
    }

    /// A removed server: its usage, its tunnel sides' and its quota go.
    pub fn forget_server(&self, server: &str) -> Result<()> {
        self.delete_where(&Of::Server(server.to_owned()))?;
        self.delete_where(&Of::ServerTunnels(server.to_owned()))?;
        let (srv, suffix) = (format!("srv:{server}"), format!("@{server}"));
        self.state()
            .cursors
            .retain(|k, _| *k != srv && !(k.starts_with("tun:") && k.ends_with(&suffix)));
        let conn = self.db.conn();
        conn.execute(
            "DELETE FROM usage_cursors WHERE subject = ?1
                OR (subject LIKE 'tun:%' AND substr(subject, -length(?2)) = ?2)",
            params![srv, suffix],
        )?;
        conn.execute("DELETE FROM usage_since WHERE subject = ?1", [&srv])?;
        conn.execute("DELETE FROM usage_quotas WHERE server = ?1", [server])?;
        Ok(())
    }

    pub fn quota(&self, server: &str) -> Result<Option<QuotaState>> {
        let quota = self
            .db
            .conn()
            .query_row(
                "SELECT limit_bytes, count, reset_day, warn FROM usage_quotas WHERE server = ?1",
                [server],
                |r| {
                    Ok(Quota {
                        limit: r.get::<_, i64>(0)?.max(0) as u64,
                        count: r.get(1)?,
                        reset_day: r.get(2)?,
                        warn: r.get(3)?,
                    })
                },
            )
            .optional()?;
        let Some(quota) = quota else {
            return Ok(None);
        };
        let settings = self.settings();
        let (start, end) = settings
            .calendar
            .period(settings.offset, quota.reset_day, now());
        let row = self.sum(&Of::Server(server.to_owned()), Some(start))?;
        Ok(Some(QuotaState {
            used: quota.used(&row),
            quota,
            period_start: start,
            period_end: end,
        }))
    }

    /// Sets a server's quota (`None` removes it). A changed quota may warn again.
    pub fn set_quota(&self, server: &str, quota: Option<&Quota>) -> Result<()> {
        let conn = self.db.conn();
        match quota {
            None => conn.execute("DELETE FROM usage_quotas WHERE server = ?1", [server])?,
            Some(q) => conn.execute(
                "INSERT INTO usage_quotas (server, limit_bytes, count, reset_day, warn)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (server) DO UPDATE SET limit_bytes = excluded.limit_bytes,
                   count = excluded.count, reset_day = excluded.reset_day,
                   warn = excluded.warn, period = 0, level = 0",
                params![server, q.limit as i64, q.count, q.reset_day, q.warn],
            )?,
        };
        Ok(())
    }

    /// The quotas that crossed their warning or their limit since this was last asked;
    /// each is told once a period.
    pub fn quota_crossings(&self) -> Result<Vec<QuotaCrossed>> {
        let servers: Vec<(String, i64, i64)> = {
            let conn = self.db.conn();
            let mut stmt = conn.prepare("SELECT server, period, level FROM usage_quotas")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.flatten().collect()
        };
        let mut out = Vec::new();
        for (server, period, level) in servers {
            let Some(state) = self.quota(&server)? else {
                continue;
            };
            let q = &state.quota;
            let now_level = if state.used >= q.limit {
                2
            } else if state.used as u128 * 100 >= q.limit as u128 * u128::from(q.warn) {
                1
            } else {
                0
            };
            let told = if period == state.period_start {
                level
            } else {
                0
            };
            if now_level > told {
                out.push(QuotaCrossed {
                    server: server.clone(),
                    used: state.used,
                    limit: q.limit,
                    full: now_level == 2,
                });
            }
            if period != state.period_start || now_level > told {
                self.db.conn().execute(
                    "UPDATE usage_quotas SET period = ?2, level = ?3 WHERE server = ?1",
                    params![server, state.period_start, now_level.max(told)],
                )?;
            }
        }
        Ok(out)
    }

    /// Every server's and tunnel's totals at once (for the lists), since each was counted
    /// and since each of `marks`; with the servers' quotas.
    pub fn summary(&self, marks: &[i64]) -> Result<Summary> {
        let all = self.by_subject(None)?;
        let by_mark: Vec<HashMap<String, Row>> = marks
            .iter()
            .map(|&m| self.by_subject(Some(m)))
            .collect::<Result<_>>()?;
        let since = self.since_all()?;
        let mut out = Summary {
            settings: self.settings(),
            servers: HashMap::new(),
            tunnels: HashMap::new(),
            sides: HashMap::new(),
        };
        let blank = || Totals {
            since: None,
            all: Row::default(),
            marks: vec![Row::default(); marks.len()],
        };
        let add = |t: &mut Totals, subject: &str| {
            if let Some(r) = all.get(subject) {
                t.all.add(r);
            }
            for (i, m) in by_mark.iter().enumerate() {
                if let Some(r) = m.get(subject) {
                    t.marks[i].add(r);
                }
            }
        };
        for subject in all.keys() {
            if let Some(id) = subject.strip_prefix("srv:") {
                let s = out
                    .servers
                    .entry(id.to_owned())
                    .or_insert_with(|| ServerSummary {
                        own: blank(),
                        tunnels: blank(),
                        quota: None,
                    });
                add(&mut s.own, subject);
            } else if let Some((name, id)) = side_of(subject) {
                let s = out
                    .servers
                    .entry(id.to_owned())
                    .or_insert_with(|| ServerSummary {
                        own: blank(),
                        tunnels: blank(),
                        quota: None,
                    });
                add(&mut s.tunnels, subject);
                let t = out.tunnels.entry(name.to_owned()).or_insert_with(blank);
                add(t, subject);
                let side = out.sides.entry(name.to_owned()).or_default();
                add(side.entry(id.to_owned()).or_insert_with(blank), subject);
            }
        }
        for (id, s) in &mut out.servers {
            s.own.since = since.get(&format!("srv:{id}")).copied();
        }
        for (name, t) in &mut out.tunnels {
            t.since = since.get(&format!("tun:{name}")).copied();
        }
        let quotas: Vec<String> = {
            let conn = self.db.conn();
            let mut stmt = conn.prepare("SELECT server FROM usage_quotas")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.flatten().collect()
        };
        for id in quotas {
            let quota = self.quota(&id)?;
            out.servers
                .entry(id)
                .or_insert_with(|| ServerSummary {
                    own: blank(),
                    tunnels: blank(),
                    quota: None,
                })
                .quota = quota;
        }
        Ok(out)
    }
}

/// What [`Usage::totals`] answers.
#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    /// Counted from (unix seconds); `None` before anything was.
    pub since: Option<i64>,
    pub all: Row,
    pub marks: Vec<Row>,
}

/// What [`Usage::summary`] answers.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub settings: Settings,
    /// By server id.
    pub servers: HashMap<String, ServerSummary>,
    /// By tunnel name, both sides together.
    pub tunnels: HashMap<String, Totals>,
    /// By tunnel name, then by server id: each side apart.
    pub sides: HashMap<String, HashMap<String, Totals>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerSummary {
    /// Its network cards.
    pub own: Totals,
    /// The tunnel sides on it.
    pub tunnels: Totals,
    pub quota: Option<QuotaState>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kariz::stats::{PeerStatus, Totals as StatTotals};
    use kariz::wire::{LegStatus, WireStatus};

    /// The `wire` counters of a leg that a test makes up.
    fn leg(sent: (u64, u64, u64, u64), received: (u64, u64, u64, u64)) -> LegStatus {
        let d = |(bytes, packets, headers, resend)| DirStatus {
            bytes,
            packets,
            headers,
            resend,
        };
        LegStatus {
            sent: d(sent),
            received: d(received),
        }
    }

    fn status(uptime: u64, up: u64, down: u64, wire: Option<WireStatus>) -> Status {
        Status {
            status_version: 1,
            kariz: "2.7.0".into(),
            role: "entry".into(),
            mode: "direct".into(),
            transport: "tcpmux".into(),
            profile: "balanced".into(),
            uptime_secs: uptime,
            peer: PeerStatus {
                transport: None,
                connected: true,
                sessions: None,
                sessions_wanted: None,
                connected_secs: None,
                rtt_ms: None,
                handshakes_ok: 0,
                handshakes_failed: 0,
                last_error: None,
                streams: None,
                streams_max: None,
                full: 0,
            },
            totals: StatTotals {
                bytes_up: up,
                bytes_down: down,
                ..StatTotals::default()
            },
            forwards: vec![],
            exit: None,
            probe: None,
            wire,
        }
    }

    /// An entry side that moved `n` bytes up: in from a user, out on the link with 10%
    /// framing, both legs with headers, and one resend on the link.
    fn entry_wire(n: u64) -> WireStatus {
        WireStatus {
            exact: true,
            user: leg((0, 0, 0, 0), (n + 520, 10, 520, 0)),
            link: leg((n + n / 10 + 520 + 1000, 10, 520, 1000), (0, 0, 0, 0)),
            target: LegStatus::default(),
            local: LegStatus::default(),
            tests: 0,
        }
    }

    #[test]
    fn a_side_is_split_into_data_overhead_resends_and_headers() {
        let d = side_raw(&status(10, 10_000, 0, Some(entry_wire(10_000))));
        let r = side_row(&d, 14, 0);
        assert_eq!(r.rx, 10_520 + 140);
        assert_eq!(r.tx, 11_000 + 520 + 1000 + 140);
        // The user's bytes on both legs; the 10% framing is the tunnel's.
        assert_eq!((r.data, r.proto, r.resend), (20_000, 1000, 1000));
        assert_eq!(r.headers, 520 + 520 + 280);
        assert_eq!(r.data + r.proto + r.resend + r.headers, r.rx + r.tx);
        assert_eq!((r.up, r.est), (10_000, 0));
        // Over a GRE link: its header on every packet of the link.
        let gre = side_row(&d, 14, 28);
        assert_eq!(gre.tx - r.tx, 280);
        // Without wire counters, an estimate.
        let old = side_row(&side_raw(&status(10, 300, 700, None)), 14, 0);
        assert_eq!((old.rx, old.tx, old.est), (1000, 1000, 2000));
    }

    #[test]
    fn readings_count_once_across_restarts() {
        let at = 1_000_000;
        let c = |start, raw: Vec<u64>| Cursor { start, at, raw };
        // The first reading only starts the count.
        assert_eq!(delta(None, at - 50, &[5]), None);
        let before = c(at - 50, vec![100, 7]);
        assert_eq!(delta(Some(&before), at - 51, &[150, 7]), Some(vec![50, 0]));
        // Restarted: all it has is new.
        assert_eq!(delta(Some(&before), at - 5, &[20, 1]), Some(vec![20, 1]));
        // Gone back without a restart: nothing this time.
        assert_eq!(delta(Some(&before), at - 50, &[90, 9]), Some(vec![0, 2]));
        // A daemon that changed its counters without restarting is not compared.
        assert_eq!(delta(Some(&before), at - 50, &[1, 2, 3]), None);
    }

    #[test]
    fn usage_is_kept_in_buckets_and_days() {
        let u = Usage::new(Db::in_memory().unwrap());
        let t = now() - now().rem_euclid(QUARTER) - 2 * QUARTER;
        let net = |rx, tx| NetTotals {
            rx_bytes: rx,
            tx_bytes: tx,
            ..NetTotals::default()
        };
        u.record_server("a", t, Some(1000), &net(5_000, 1_000));
        u.record_server("a", t + 10, Some(1010), &net(6_000, 1_500));
        u.record_side(
            "web",
            "a",
            t + 10,
            &status(100, 0, 0, Some(entry_wire(0))),
            14,
            Some(0),
        );
        u.record_side(
            "web",
            "a",
            t + 20,
            &status(110, 10_000, 0, Some(entry_wire(10_000))),
            14,
            Some(0),
        );
        // Before it is written, and after.
        let of = Of::Server("a".into());
        assert_eq!(u.totals(&of, &[]).unwrap().all.rx, 1000);
        u.flush().unwrap();
        u.record_server(
            "a",
            t + QUARTER,
            Some(1000 + QUARTER as u64),
            &net(7_000, 1_500),
        );
        let series = u.series(&of, t - 3600, t + 3600, false).unwrap();
        assert_eq!(
            series
                .iter()
                .map(|(ts, r)| (*ts, r.rx, r.tx))
                .collect::<Vec<_>>(),
            vec![(t, 1000, 500), (t + QUARTER, 1000, 0)]
        );
        u.flush().unwrap();
        let totals = u.totals(&of, &[t + 1]).unwrap();
        assert_eq!((totals.all.rx, totals.marks[0].rx), (2000, 1000));
        assert_eq!(totals.since, Some(t));
        let web = u.totals(&Of::Tunnel("web".into()), &[]).unwrap();
        assert_eq!(web.all.data, 20_000);
        let on_a = u.totals(&Of::ServerTunnels("a".into()), &[]).unwrap();
        assert_eq!(on_a.all, web.all);
        assert_eq!(u.tunnels_on("a").unwrap(), vec!["web".to_string()]);

        // A panel that restarts picks up from the saved readings.
        let again = Usage::new(u.db.clone());
        again.record_server(
            "a",
            t + 2 * QUARTER,
            Some(1000 + 2 * QUARTER as u64),
            &net(7_500, 1_500),
        );
        again.flush().unwrap();
        assert_eq!(again.totals(&of, &[]).unwrap().all.rx, 2500);

        // A reset starts from now; deleting a tunnel takes its usage.
        again.reset_server("a", false).unwrap();
        assert_eq!(again.totals(&of, &[]).unwrap().all, Row::default());
        assert!(again.totals(&of, &[]).unwrap().since.unwrap() >= t);
        assert_eq!(
            again
                .totals(&Of::Tunnel("web".into()), &[])
                .unwrap()
                .all
                .data,
            20_000
        );
        again.forget_tunnel("web").unwrap();
        let gone = again.totals(&Of::Tunnel("web".into()), &[]).unwrap();
        assert_eq!((gone.all, gone.since), (Row::default(), None));
    }

    #[test]
    fn a_quota_warns_once_and_again_at_its_limit() {
        let u = Usage::new(Db::in_memory().unwrap());
        let q = Quota {
            limit: 10_000,
            count: "tx".into(),
            reset_day: 1,
            warn: 80,
        };
        assert!(q.valid());
        u.set_quota("a", Some(&q)).unwrap();
        let at = now();
        let net = |tx| NetTotals {
            tx_bytes: tx,
            rx_bytes: 50_000,
            ..NetTotals::default()
        };
        u.record_server("a", at, Some(1000), &net(0));
        u.record_server("a", at, Some(1000), &net(5_000));
        assert!(u.quota_crossings().unwrap().is_empty());
        u.record_server("a", at, Some(1000), &net(8_500));
        let told = u.quota_crossings().unwrap();
        assert_eq!(told.len(), 1);
        assert!(!told[0].full);
        assert!(u.quota_crossings().unwrap().is_empty());
        u.record_server("a", at, Some(1000), &net(10_000));
        let told = u.quota_crossings().unwrap();
        assert!(told[0].full);
        assert_eq!(told[0].used, 10_000);
        assert!(u.quota_crossings().unwrap().is_empty());
        let state = u.quota("a").unwrap().unwrap();
        assert!(state.period_start <= at && at < state.period_end);
        u.set_quota("a", None).unwrap();
        assert!(u.quota("a").unwrap().is_none());
    }
}
