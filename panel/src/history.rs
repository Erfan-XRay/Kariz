//! What the panel remembers about the servers and tunnels: a time series per measure for
//! the charts, and the events (a tunnel going down, a server going offline) for the log.
//!
//! The last hour is kept in memory at the rate the servers are asked (every couple of
//! seconds); five-minute averages go to SQLite and are kept for 30 days.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::params;
use serde::Serialize;

use crate::auth::now;
use crate::db::Db;

/// Seconds one stored average covers.
pub const BUCKET: i64 = 300;
/// How long averages are kept.
pub const KEEP_SECS: i64 = 30 * 24 * 3600;
/// Samples kept in memory per series (an hour at one every two seconds).
const RING: usize = 1800;
/// Most points a chart is given.
const MAX_POINTS: usize = 360;

/// How far back a chart looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hour,
    Day,
    Week,
    Month,
}

impl Range {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "1h" => Self::Hour,
            "24h" => Self::Day,
            "7d" => Self::Week,
            "30d" => Self::Month,
            _ => return None,
        })
    }

    pub fn secs(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 24 * 3600,
            Self::Week => 7 * 24 * 3600,
            Self::Month => 30 * 24 * 3600,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    pub id: i64,
    pub at: i64,
    /// `server_up`, `server_down`, `tunnel_up`, `tunnel_down`, `change`.
    pub kind: String,
    /// The server or tunnel it is about.
    pub subject: String,
    pub detail: String,
}

#[derive(Default)]
struct Bucket {
    start: i64,
    sum: f64,
    n: u32,
}

pub struct History {
    db: Db,
    ring: Mutex<HashMap<String, VecDeque<(i64, f64)>>>,
    buckets: Mutex<HashMap<String, Bucket>>,
}

/// A series key: `srv:ID:cpu`, `tun:NAME:rate`. Keys come from the browser, so they are
/// limited to plain characters.
pub fn valid_key(key: &str) -> bool {
    key.len() <= 96
        && (key.starts_with("srv:") || key.starts_with("tun:"))
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_' | b'.'))
}

impl History {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            ring: Mutex::new(HashMap::new()),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// One reading of a measure, at `at` (unix seconds).
    pub fn record(&self, key: &str, at: i64, value: f64) {
        if !value.is_finite() {
            return;
        }
        {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            let series = ring.entry(key.to_owned()).or_default();
            if series.back().map_or(true, |&(t, _)| at > t) {
                if series.len() >= RING {
                    series.pop_front();
                }
                series.push_back((at, value));
            }
        }
        let start = at - at.rem_euclid(BUCKET);
        let done = {
            let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
            let b = buckets.entry(key.to_owned()).or_insert_with(|| Bucket {
                start,
                ..Bucket::default()
            });
            let finished = (b.start != start && b.n > 0).then(|| (b.start, b.sum / f64::from(b.n)));
            if b.start != start {
                *b = Bucket {
                    start,
                    sum: 0.0,
                    n: 0,
                };
            }
            b.sum += value;
            b.n += 1;
            finished
        };
        if let Some((ts, avg)) = done {
            let _ = self.db.conn().execute(
                "INSERT OR REPLACE INTO metrics (key, ts, v) VALUES (?1, ?2, ?3)",
                params![key, ts, avg],
            );
        }
    }

    /// The points of a series over `range`, at most a few hundred, oldest first.
    pub fn series(&self, key: &str, range: Range) -> Result<Vec<(i64, f64)>> {
        let since = now() - range.secs();
        let mut points: Vec<(i64, f64)> = if range == Range::Hour {
            let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            ring.get(key)
                .map(|s| s.iter().copied().filter(|&(t, _)| t >= since).collect())
                .unwrap_or_default()
        } else {
            let conn = self.db.conn();
            let mut stmt = conn.prepare_cached(
                "SELECT ts, v FROM metrics WHERE key = ?1 AND ts >= ?2 ORDER BY ts",
            )?;
            let rows = stmt.query_map(params![key, since], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(thin(&mut points))
    }

    pub fn prune(&self) -> Result<()> {
        let conn = self.db.conn();
        conn.execute("DELETE FROM metrics WHERE ts < ?1", [now() - KEEP_SECS])?;
        conn.execute("DELETE FROM events WHERE ts < ?1", [now() - KEEP_SECS])?;
        // The audit log is kept for a year.
        conn.execute("DELETE FROM audit WHERE at < ?1", [now() - 365 * 24 * 3600])?;
        Ok(())
    }

    pub fn event(&self, kind: &str, subject: &str, detail: &str) {
        let _ = self.db.conn().execute(
            "INSERT INTO events (ts, kind, subject, detail) VALUES (?1, ?2, ?3, ?4)",
            params![now(), kind, subject, detail],
        );
    }

    /// The newest events first.
    pub fn events(&self, limit: u32) -> Result<Vec<Event>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, ts, kind, subject, detail FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.min(500)], |r| {
            Ok(Event {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                subject: r.get(3)?,
                detail: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Averages neighbours together until at most [`MAX_POINTS`] are left.
fn thin(points: &mut Vec<(i64, f64)>) -> Vec<(i64, f64)> {
    if points.len() <= MAX_POINTS {
        return std::mem::take(points);
    }
    let step = points.len().div_ceil(MAX_POINTS);
    points
        .chunks(step)
        .map(|c| {
            let t = c[c.len() / 2].0;
            (t, c.iter().map(|p| p.1).sum::<f64>() / c.len() as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> History {
        History::new(Db::in_memory().unwrap())
    }

    #[test]
    fn the_last_hour_comes_from_memory_in_order() {
        let h = history();
        let t = now();
        for i in 0..10 {
            h.record("srv:a:cpu", t - 20 + i * 2, i as f64);
        }
        // a sample that is not newer than the last is ignored
        h.record("srv:a:cpu", t - 20, 99.0);
        let s = h.series("srv:a:cpu", Range::Hour).unwrap();
        assert_eq!(s.len(), 10);
        assert!(s.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(h.series("srv:none:cpu", Range::Hour).unwrap().is_empty());
    }

    #[test]
    fn five_minute_averages_are_stored_when_a_bucket_ends() {
        let h = history();
        let base = now() - now().rem_euclid(BUCKET) - 2 * BUCKET;
        h.record("tun:x:rate", base + 10, 10.0);
        h.record("tun:x:rate", base + 20, 30.0);
        // nothing is stored until the bucket is over
        assert!(h.series("tun:x:rate", Range::Day).unwrap().is_empty());
        h.record("tun:x:rate", base + BUCKET + 5, 50.0);
        let s = h.series("tun:x:rate", Range::Day).unwrap();
        assert_eq!(s, vec![(base, 20.0)]);
    }

    #[test]
    fn long_series_are_thinned_and_old_data_is_pruned() {
        let mut points: Vec<(i64, f64)> = (0..1000).map(|i| (i, 1.0)).collect();
        let thin = thin(&mut points);
        assert!(thin.len() <= MAX_POINTS && thin.iter().all(|p| p.1 == 1.0));
        let h = history();
        h.db.conn()
            .execute(
                "INSERT INTO metrics (key, ts, v) VALUES ('srv:a:cpu', ?1, 1.0)",
                [now() - KEEP_SECS - 10],
            )
            .unwrap();
        h.event("tunnel_down", "demo", "x");
        h.prune().unwrap();
        let left: i64 =
            h.db.conn()
                .query_row("SELECT COUNT(*) FROM metrics", [], |r| r.get(0))
                .unwrap();
        assert_eq!(left, 0);
        assert_eq!(h.events(10).unwrap().len(), 1, "recent events stay");
    }

    #[test]
    fn events_come_newest_first_and_keys_are_checked() {
        let h = history();
        h.event("server_down", "frankfurt", "");
        h.event("server_up", "frankfurt", "");
        let e = h.events(10).unwrap();
        assert_eq!(e[0].kind, "server_up");
        assert_eq!(e.len(), 2);
        assert!(valid_key("srv:local:cpu") && valid_key("tun:pair-1:rate"));
        assert!(!valid_key("meta:link_token") && !valid_key("srv:a b") && !valid_key("srv:'; --"));
    }
}
