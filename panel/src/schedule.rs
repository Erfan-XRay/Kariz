//! Automatic restarts. A tunnel, or every running tunnel of a server, is restarted on a timer
//! the user sets in the panel: every so many minutes or hours, or every day at a time.
//!
//! The panel keeps the timers and does the restarts itself (there is no cron on the servers),
//! through the same requests as the Restart button, so any server whose agent is connected
//! can have one. A server that is offline is not restarted: it is tried again as soon as it is
//! back (for a daily time, within the hour). A tunnel that was stopped on purpose stays
//! stopped: only what is running is restarted.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::auth::now;
use crate::db::Db;
use crate::hub::Hub;
use crate::manage::valid_name;
use crate::pair;
use std::sync::Arc;

/// The shortest interval: a restart drops the tunnel for a moment.
pub const MIN_EVERY: i64 = 600;
/// The longest interval.
pub const MAX_EVERY: i64 = 30 * 86_400;
/// A daily time the panel missed by more than this (it was not running) is skipped, not made up.
const GRACE: i64 = 3600;
/// How often the panel looks at the list.
const TICK: Duration = Duration::from_secs(20);
/// A pause between two tunnels of a server, so they do not all drop at once.
const GAP: Duration = Duration::from_secs(2);

/// What is running now, by `(kind, subject)`: never started twice.
static BUSY: LazyLock<Mutex<HashSet<(String, String)>>> = LazyLock::new(Mutex::default);

fn busy() -> std::sync::MutexGuard<'static, HashSet<(String, String)>> {
    BUSY.lock().unwrap_or_else(|e| e.into_inner())
}

/// One timer, as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    /// `tunnel` or `server`.
    pub kind: String,
    /// The tunnel's name, or the server's id.
    pub subject: String,
    /// `every` or `daily`.
    pub mode: String,
    pub every_secs: i64,
    /// For `daily`: minutes after midnight, UTC.
    pub daily_min: i64,
    pub enabled: bool,
    /// When it was set (unix seconds).
    pub since: i64,
    /// When it last ran (0: never).
    pub last_run: i64,
    /// `ok`, `ok:N`, `skipped_offline`, `skipped_stopped`, `skipped_none`, `busy`, `missed`,
    /// or `failed:` and why.
    pub last_result: String,
    /// When it runs next (unix seconds); none when it is switched off.
    pub next_run: Option<i64>,
}

/// What the browser sends to set a timer.
#[derive(Debug, Clone, Deserialize)]
pub struct NewSchedule {
    pub kind: String,
    pub subject: String,
    pub mode: String,
    #[serde(default)]
    pub every_secs: i64,
    #[serde(default)]
    pub daily_min: i64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// When a timer runs next, counted from the last run (or from when it was set).
pub fn next_run(s: &Schedule) -> Option<i64> {
    if !s.enabled {
        return None;
    }
    let base = s.last_run.max(s.since);
    match s.mode.as_str() {
        "every" => Some(base + s.every_secs.clamp(MIN_EVERY, MAX_EVERY)),
        "daily" => {
            let slot = base - base.rem_euclid(86_400) + s.daily_min.clamp(0, 1439) * 60;
            Some(if slot > base { slot } else { slot + 86_400 })
        }
        _ => None,
    }
}

pub fn list(db: &Db) -> Result<Vec<Schedule>> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT kind, subject, mode, every_secs, daily_min, enabled, since, last_run, last_result \
         FROM schedules ORDER BY kind, subject",
    )?;
    let rows = stmt.query_map([], |r| {
        let mut s = Schedule {
            kind: r.get(0)?,
            subject: r.get(1)?,
            mode: r.get(2)?,
            every_secs: r.get(3)?,
            daily_min: r.get(4)?,
            enabled: r.get::<_, i64>(5)? != 0,
            since: r.get(6)?,
            last_run: r.get(7)?,
            last_result: r.get(8)?,
            next_run: None,
        };
        s.next_run = next_run(&s);
        Ok(s)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Sets (or changes) a timer. It counts from now: a daily time that has passed today waits
/// for tomorrow.
pub fn set(hub: &Hub, new: &NewSchedule) -> Result<()> {
    let servers = hub.snapshot()?;
    match new.kind.as_str() {
        "tunnel" => {
            if !valid_name(&new.subject) {
                bail!("bad_name");
            }
            if !servers
                .iter()
                .any(|s| s.tunnels.iter().any(|t| t.info.name == new.subject))
            {
                bail!("no_such_tunnel");
            }
        }
        "server" => {
            if !servers.iter().any(|s| s.id == new.subject) {
                bail!("no_such_server");
            }
        }
        _ => bail!("bad_input"),
    }
    match new.mode.as_str() {
        "every" if (MIN_EVERY..=MAX_EVERY).contains(&new.every_secs) => {}
        "daily" if (0..1440).contains(&new.daily_min) => {}
        _ => bail!("bad_input"),
    }
    hub.db.conn().execute(
        "INSERT INTO schedules (kind, subject, mode, every_secs, daily_min, enabled, since, last_run, last_result) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, '') \
         ON CONFLICT (kind, subject) DO UPDATE SET mode = ?3, every_secs = ?4, daily_min = ?5, \
         enabled = ?6, since = ?7, last_run = 0, last_result = ''",
        params![
            new.kind,
            new.subject,
            new.mode,
            if new.mode == "every" { new.every_secs } else { 0 },
            if new.mode == "daily" { new.daily_min } else { 0 },
            i64::from(new.enabled),
            now()
        ],
    )?;
    Ok(())
}

pub fn delete(db: &Db, kind: &str, subject: &str) -> Result<bool> {
    Ok(db.conn().execute(
        "DELETE FROM schedules WHERE kind = ?1 AND subject = ?2",
        params![kind, subject],
    )? > 0)
}

/// Restarts a tunnel from both sides, in the order that never leaves a dialer without its
/// acceptor (the Restart button's own operation).
async fn restart_tunnel(hub: &Arc<Hub>, name: &str) -> String {
    let Ok(servers) = hub.snapshot() else {
        return "failed:database".into();
    };
    let sides: Vec<_> = servers
        .iter()
        .filter_map(|s| {
            s.tunnels
                .iter()
                .find(|t| t.info.name == name)
                .map(|t| (s.online, t.info.active))
        })
        .collect();
    if sides.is_empty() {
        return "skipped_none".into();
    }
    if sides.iter().all(|(_, active)| *active == Some(false)) {
        return "skipped_stopped".into();
    }
    if sides.iter().all(|(online, _)| !online) {
        return "skipped_offline".into();
    }
    let op = match pair::control(hub, name, "restart") {
        Ok(op) => op,
        Err(e) => {
            let e = format!("{e:#}");
            return if e == "busy" {
                "busy".into()
            } else {
                format!("failed:{e}")
            };
        }
    };
    // The operation runs in the background: wait for it, up to two minutes.
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        match hub.ops.get(&op) {
            Some(o) if o.state == "running" => {}
            Some(o) => {
                return o
                    .error
                    .map_or_else(|| "ok".into(), |e| format!("failed:{e}"))
            }
            None => return "ok".into(),
        }
    }
    "failed:timeout".into()
}

/// Restarts every running tunnel of one server, one after the other. The agent itself is
/// not restarted (its link would end), and neither is a tunnel that was stopped.
async fn restart_server(hub: &Arc<Hub>, id: &str) -> String {
    let Ok(servers) = hub.snapshot() else {
        return "failed:database".into();
    };
    let Some(server) = servers.into_iter().find(|s| s.id == id) else {
        return "failed:no_such_server".into();
    };
    if !server.online {
        return "skipped_offline".into();
    }
    let names: Vec<String> = server
        .tunnels
        .iter()
        .filter(|t| t.info.active != Some(false) && t.info.error.is_none())
        .map(|t| t.info.name.clone())
        .collect();
    if names.is_empty() {
        return "skipped_none".into();
    }
    let (mut done, mut failed) = (0usize, Vec::new());
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(GAP).await;
        }
        match pair::ctl(hub, id, name, "restart").await {
            Ok(()) => done += 1,
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if failed.is_empty() {
        format!("ok:{done}")
    } else {
        format!("failed:{}", failed.join("; "))
    }
}

async fn execute(hub: &Arc<Hub>, kind: &str, subject: &str) -> String {
    match kind {
        "tunnel" => restart_tunnel(hub, subject).await,
        "server" => restart_server(hub, subject).await,
        _ => "failed:bad_input".into(),
    }
}

/// Records how a run went. A server that was offline keeps its place in line: it is tried
/// again at the next look.
fn finish(hub: &Hub, kind: &str, subject: &str, started: i64, result: &str) {
    let keep_turn = result == "skipped_offline" || result == "busy";
    let _ = hub.db.conn().execute(
        if keep_turn {
            "UPDATE schedules SET last_result = ?4 WHERE kind = ?1 AND subject = ?2 AND ?3 > 0"
        } else {
            "UPDATE schedules SET last_run = ?3, last_result = ?4 WHERE kind = ?1 AND subject = ?2"
        },
        params![kind, subject, started, result],
    );
    if result.starts_with("ok") || result.starts_with("failed") {
        let (event, who) = if kind == "tunnel" {
            ("auto_restart", subject)
        } else {
            ("auto_restart_server", subject)
        };
        hub.history.event(event, who, result);
        let _ = hub.db.audit(
            "scheduler",
            None,
            &format!("restart of {kind} {subject}: {result}"),
        );
    }
    info!(%kind, %subject, %result, "an automatic restart ran");
}

/// Runs one timer's restart now, in the background (the page reads the result from the list).
/// False if it is already running.
pub fn run_now(hub: &Arc<Hub>, kind: &str, subject: &str) -> bool {
    let key = (kind.to_owned(), subject.to_owned());
    if !busy().insert(key.clone()) {
        return false;
    }
    let hub = hub.clone();
    tokio::spawn(async move {
        let started = now();
        let result = execute(&hub, &key.0, &key.1).await;
        finish(&hub, &key.0, &key.1, started, &result);
        busy().remove(&key);
    });
    true
}

/// Looks at the timers every few seconds, for ever, and runs the ones that are due.
pub async fn run(hub: Arc<Hub>) {
    loop {
        tokio::time::sleep(TICK).await;
        let Ok(all) = list(&hub.db) else { continue };
        let t = now();
        for s in all {
            let Some(next) = s.next_run else { continue };
            if next > t {
                continue;
            }
            // A daily time the panel slept through is not made up for hours later.
            if s.mode == "daily" && t - next > GRACE {
                let _ = hub.db.conn().execute(
                    "UPDATE schedules SET last_run = ?3, last_result = 'missed' WHERE kind = ?1 AND subject = ?2",
                    params![s.kind, s.subject, next],
                );
                continue;
            }
            debug!(kind = %s.kind, subject = %s.subject, "an automatic restart is due");
            run_now(&hub, &s.kind, &s.subject);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(mode: &str, every: i64, daily: i64, since: i64, last: i64) -> Schedule {
        Schedule {
            kind: "tunnel".into(),
            subject: "main".into(),
            mode: mode.into(),
            every_secs: every,
            daily_min: daily,
            enabled: true,
            since,
            last_run: last,
            last_result: String::new(),
            next_run: None,
        }
    }

    #[test]
    fn an_interval_counts_from_the_last_run_or_from_when_it_was_set() {
        assert_eq!(next_run(&sched("every", 3600, 0, 1000, 0)), Some(4600));
        assert_eq!(next_run(&sched("every", 3600, 0, 1000, 9000)), Some(12600));
        // Never shorter than ten minutes, whatever the database holds.
        assert_eq!(next_run(&sched("every", 5, 0, 1000, 0)), Some(1600));
    }

    #[test]
    fn a_daily_time_is_the_next_one_after_the_last_run() {
        // 03:30 UTC; the timer was set at day 10, 12:00.
        let day = 10 * 86_400;
        let set_at = day + 12 * 3600;
        let a = sched("daily", 0, 3 * 60 + 30, set_at, 0);
        assert_eq!(next_run(&a), Some(day + 86_400 + 3 * 3600 + 1800));
        // After it ran a little late the next morning, the next one is the day after.
        let ran = day + 86_400 + 3 * 3600 + 1805;
        let b = sched("daily", 0, 3 * 60 + 30, set_at, ran);
        assert_eq!(next_run(&b), Some(day + 2 * 86_400 + 3 * 3600 + 1800));
        // Set before the time on the same day: it runs today.
        let early = sched("daily", 0, 18 * 60, set_at, 0);
        assert_eq!(next_run(&early), Some(day + 18 * 3600));
    }

    #[test]
    fn a_switched_off_timer_never_runs() {
        let mut s = sched("every", 3600, 0, 0, 0);
        s.enabled = false;
        assert_eq!(next_run(&s), None);
        assert_eq!(next_run(&sched("weekly", 0, 0, 0, 0)), None);
    }
}
