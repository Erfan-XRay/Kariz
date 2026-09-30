//! Sign-in: the admin password, one-time login links, sessions and the lockout after
//! failed tries.
//!
//! Everything here takes `now` (unix seconds) so tests can move time, and returns plain
//! values; the HTTP side is in `api.rs`.
//!
//! Secrets are never stored as they are: the password as an Argon2id hash, and every
//! token (a login link, a session) as a BLAKE3 hash of its 32 random bytes. A stolen
//! database therefore opens nothing.

use anyhow::{anyhow, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rusqlite::{params, OptionalExtension};

use crate::db::Db;

/// A login link works for this long, once.
pub const LINK_TTL: i64 = 3600;
/// A session ends after this long without a request.
pub const SESSION_IDLE: i64 = 7 * 86_400;
/// At most this many sessions; a new one beyond it ends the oldest.
pub const MAX_SESSIONS: i64 = 30;
/// This many failed tries from one address...
pub const MAX_FAILURES: i64 = 5;
/// ...lock it out for this long after the last one.
pub const LOCKOUT: i64 = 900;
/// Shortest password.
pub const MIN_PASSWORD: usize = 10;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// 32 random bytes as 64 hex characters.
pub fn new_token() -> Result<String> {
    crate::config::random_hex(32)
}

/// What is stored of a token.
pub fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn argon2() -> Argon2<'static> {
    // The OWASP baseline for Argon2id: 19 MiB, 2 passes, 1 lane.
    let params = Params::new(19 * 1024, 2, 1, None).expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// The Argon2id hash of a password, as a PHC string.
pub fn hash_password(password: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow!("no randomness: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow!("{e}"))?;
    Ok(argon2()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("{e}"))?
        .to_string())
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .is_ok_and(|hash| argon2().verify_password(password.as_bytes(), &hash).is_ok())
}

// ---- the password ----

pub fn has_password(db: &Db) -> Result<bool> {
    Ok(db.meta("password_hash")?.is_some())
}

/// Sets the password and ends every session except `keep` (a session id).
pub fn set_password(db: &Db, password: &str, keep: Option<i64>) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        anyhow::bail!("the password needs at least {MIN_PASSWORD} characters");
    }
    db.set_meta("password_hash", &hash_password(password)?)?;
    db.conn()
        .execute("DELETE FROM sessions WHERE id IS NOT ?1", params![keep])?;
    Ok(())
}

pub fn remove_password(db: &Db) -> Result<()> {
    db.conn()
        .execute("DELETE FROM meta WHERE key = 'password_hash'", [])?;
    Ok(())
}

/// Whether `password` is the admin password. A panel with none never matches.
pub fn check_password(db: &Db, password: &str) -> Result<bool> {
    Ok(match db.meta("password_hash")? {
        Some(phc) => verify_password(password, &phc),
        None => false,
    })
}

// ---- one-time login links ----

/// Makes a login link token, valid for [`LINK_TTL`].
pub fn create_link(db: &Db, now: i64) -> Result<String> {
    let token = new_token()?;
    db.conn().execute(
        "INSERT INTO login_links (hash, created, expires, used) VALUES (?1, ?2, ?3, 0)",
        params![hash_token(&token), now, now + LINK_TTL],
    )?;
    Ok(token)
}

/// Spends a login link: true once for a token that exists, is not expired and was not
/// used. The update is one statement, so two tries at the same moment cannot both win.
pub fn spend_link(db: &Db, token: &str, now: i64) -> Result<bool> {
    let conn = db.conn();
    conn.execute("DELETE FROM login_links WHERE expires < ?1 - 86400", [now])?;
    let changed = conn.execute(
        "UPDATE login_links SET used = 1 WHERE hash = ?1 AND used = 0 AND expires > ?2",
        params![hash_token(token), now],
    )?;
    Ok(changed == 1)
}

// ---- lockout ----

/// Seconds left of a lockout for `ip`, if it is locked out.
pub fn locked_out(db: &Db, ip: &str, now: i64) -> Result<Option<i64>> {
    let conn = db.conn();
    let (count, last): (i64, Option<i64>) = conn.query_row(
        "SELECT count(*), max(at) FROM failures WHERE ip = ?1 AND at > ?2",
        params![ip, now - LOCKOUT],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(match last {
        Some(last) if count >= MAX_FAILURES => Some((last + LOCKOUT - now).max(1)),
        _ => None,
    })
}

/// Records a failed try; returns how many tries are left before the lockout.
pub fn record_failure(db: &Db, ip: &str, now: i64) -> Result<i64> {
    let conn = db.conn();
    conn.execute("DELETE FROM failures WHERE at < ?1", [now - LOCKOUT])?;
    conn.execute(
        "INSERT INTO failures (ip, at) VALUES (?1, ?2)",
        params![ip, now],
    )?;
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM failures WHERE ip = ?1 AND at > ?2",
        params![ip, now - LOCKOUT],
        |r| r.get(0),
    )?;
    Ok((MAX_FAILURES - count).max(0))
}

pub fn clear_failures(db: &Db, ip: &str) -> Result<()> {
    db.conn()
        .execute("DELETE FROM failures WHERE ip = ?1", [ip])?;
    Ok(())
}

// ---- sessions ----

pub struct NewSession {
    /// The cookie's value; only its hash is stored.
    pub token: String,
    pub csrf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    pub csrf: String,
    pub ip: String,
    pub agent: String,
    pub created: i64,
    pub last_seen: i64,
}

pub fn create_session(db: &Db, ip: &str, agent: &str, now: i64) -> Result<NewSession> {
    let (token, csrf) = (new_token()?, new_token()?);
    let conn = db.conn();
    conn.execute(
        "INSERT INTO sessions (hash, csrf, created, last_seen, ip, agent)
         VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
        params![
            hash_token(&token),
            csrf,
            now,
            ip,
            agent.chars().take(200).collect::<String>()
        ],
    )?;
    // Beyond the limit, the least recently used ones go.
    conn.execute(
        "DELETE FROM sessions WHERE id NOT IN
         (SELECT id FROM sessions ORDER BY last_seen DESC, id DESC LIMIT ?1)",
        [MAX_SESSIONS],
    )?;
    Ok(NewSession { token, csrf })
}

/// The session of a cookie value, refreshed; `None` when there is none or it went idle.
pub fn find_session(db: &Db, token: &str, now: i64) -> Result<Option<Session>> {
    let conn = db.conn();
    conn.execute(
        "DELETE FROM sessions WHERE last_seen < ?1",
        [now - SESSION_IDLE],
    )?;
    let hash = hash_token(token);
    let session = conn
        .query_row(
            "SELECT id, csrf, ip, agent, created, last_seen FROM sessions WHERE hash = ?1",
            [&hash],
            row_to_session,
        )
        .optional()?;
    if let Some(s) = &session {
        // Written at most once a minute: reading a page should not be a write.
        if now - s.last_seen >= 60 {
            conn.execute(
                "UPDATE sessions SET last_seen = ?1 WHERE id = ?2",
                params![now, s.id],
            )?;
        }
    }
    Ok(session)
}

fn row_to_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        csrf: r.get(1)?,
        ip: r.get(2)?,
        agent: r.get(3)?,
        created: r.get(4)?,
        last_seen: r.get(5)?,
    })
}

pub fn list_sessions(db: &Db) -> Result<Vec<Session>> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT id, csrf, ip, agent, created, last_seen FROM sessions ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map([], row_to_session)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn revoke_session(db: &Db, id: i64) -> Result<bool> {
    Ok(db
        .conn()
        .execute("DELETE FROM sessions WHERE id = ?1", [id])?
        == 1)
}

pub fn revoke_all(db: &Db) -> Result<()> {
    db.conn().execute("DELETE FROM sessions", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::in_memory().unwrap()
    }

    #[test]
    fn passwords_are_hashed_checked_and_length_limited() {
        let db = db();
        assert!(!has_password(&db).unwrap());
        assert!(!check_password(&db, "anything at all").unwrap());
        assert!(set_password(&db, "short", None).is_err());

        set_password(&db, "a long enough password", None).unwrap();
        assert!(has_password(&db).unwrap());
        assert!(check_password(&db, "a long enough password").unwrap());
        assert!(!check_password(&db, "a long enough passwor").unwrap());
        let phc = db.meta("password_hash").unwrap().unwrap();
        assert!(phc.starts_with("$argon2id$"), "{phc}");
        assert!(!phc.contains("long enough"));
        // The same password hashed twice differs (a random salt).
        assert_ne!(
            hash_password("same password").unwrap(),
            hash_password("same password").unwrap()
        );

        remove_password(&db).unwrap();
        assert!(!has_password(&db).unwrap());
    }

    #[test]
    fn a_login_link_works_once_and_expires() {
        let db = db();
        let t = 1_000_000;
        let token = create_link(&db, t).unwrap();
        assert_eq!(token.len(), 64);
        // Only its hash is stored.
        let stored: String = db
            .conn()
            .query_row("SELECT hash FROM login_links", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, token);
        assert_eq!(stored, hash_token(&token));

        assert!(!spend_link(&db, "not a token", t).unwrap());
        assert!(spend_link(&db, &token, t + 10).unwrap());
        assert!(!spend_link(&db, &token, t + 11).unwrap(), "spent");

        let late = create_link(&db, t).unwrap();
        assert!(
            !spend_link(&db, &late, t + LINK_TTL + 1).unwrap(),
            "expired"
        );
        let edge = create_link(&db, t).unwrap();
        assert!(spend_link(&db, &edge, t + LINK_TTL - 1).unwrap());
    }

    #[test]
    fn five_failures_lock_an_address_for_fifteen_minutes() {
        let db = db();
        let t = 5_000_000;
        assert_eq!(locked_out(&db, "1.2.3.4", t).unwrap(), None);
        for i in 0..4 {
            assert_eq!(record_failure(&db, "1.2.3.4", t + i).unwrap(), 4 - i);
            assert_eq!(locked_out(&db, "1.2.3.4", t + i).unwrap(), None);
        }
        assert_eq!(record_failure(&db, "1.2.3.4", t + 4).unwrap(), 0);
        assert_eq!(
            locked_out(&db, "1.2.3.4", t + 5).unwrap(),
            Some(LOCKOUT - 1)
        );
        // Others are not affected.
        assert_eq!(locked_out(&db, "5.6.7.8", t + 5).unwrap(), None);
        // It ends by itself.
        assert_eq!(locked_out(&db, "1.2.3.4", t + 4 + LOCKOUT).unwrap(), None);
        // A success clears the count.
        clear_failures(&db, "1.2.3.4").unwrap();
        assert_eq!(record_failure(&db, "1.2.3.4", t + 6).unwrap(), 4);
    }

    #[test]
    fn sessions_are_found_refreshed_limited_and_revoked() {
        let db = db();
        let t = 9_000_000;
        let a = create_session(&db, "1.1.1.1", "Firefox", t).unwrap();
        assert_ne!(a.token, a.csrf);
        let stored: String = db
            .conn()
            .query_row("SELECT hash FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, a.token);

        let s = find_session(&db, &a.token, t + 5).unwrap().unwrap();
        assert_eq!(
            (s.ip.as_str(), s.agent.as_str(), s.csrf.as_str()),
            ("1.1.1.1", "Firefox", a.csrf.as_str())
        );
        assert!(find_session(&db, "wrong", t).unwrap().is_none());

        // Refreshed at most once a minute, and idle sessions end.
        find_session(&db, &a.token, t + 100).unwrap();
        assert_eq!(list_sessions(&db).unwrap()[0].last_seen, t + 100);
        assert!(find_session(&db, &a.token, t + 100 + SESSION_IDLE + 1)
            .unwrap()
            .is_none());

        // The limit ends the least recently used.
        let first = create_session(&db, "2.2.2.2", "first", t).unwrap();
        for i in 0..MAX_SESSIONS {
            create_session(&db, "3.3.3.3", "later", t + 1 + i).unwrap();
        }
        assert_eq!(list_sessions(&db).unwrap().len() as i64, MAX_SESSIONS);
        assert!(find_session(&db, &first.token, t + 40).unwrap().is_none());

        // Revoking, and changing the password ends the others.
        let keep = create_session(&db, "4.4.4.4", "me", t + 50).unwrap();
        let me = find_session(&db, &keep.token, t + 50).unwrap().unwrap();
        set_password(&db, "another long password", Some(me.id)).unwrap();
        let left = list_sessions(&db).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, me.id);
        assert!(revoke_session(&db, me.id).unwrap());
        assert!(!revoke_session(&db, me.id).unwrap());
    }
}
