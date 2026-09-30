//! A backup of the panel: the servers it knows (with the keys they prove themselves with)
//! and the link token they dial with, in one file locked with a passphrase
//! (docs/PHASE12.md, section 5).
//!
//! Tunnels are not in it: they live on the servers, in their own files, and the panel
//! reads them from there. Sessions and login links are not in it either.
//!
//! The file is `KZB1`, a random salt, a random nonce, then the JSON sealed with
//! ChaCha20-Poly1305 under a key that Argon2id makes from the passphrase.

use anyhow::{anyhow, bail, Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::auth::now;
use crate::db::Db;

const MAGIC: &[u8; 4] = b"KZB1";
pub const MIN_PASSPHRASE: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
struct Server {
    id: String,
    name: String,
    key: String,
    created: i64,
    last_seen: i64,
    version: String,
    arch: String,
    host: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    format: u32,
    created: i64,
    panel: String,
    link_token: Option<String>,
    servers: Vec<Server>,
}

fn derive(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = argon2::Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(key)
}

fn sealing_key(key: &[u8; 32]) -> Result<LessSafeKey> {
    Ok(LessSafeKey::new(
        UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| anyhow!("bad key"))?,
    ))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| anyhow!("no randomness: {e}"))?;
    Ok(b)
}

/// The backup file for `db`, locked with `passphrase`.
pub fn export(db: &Db, passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.chars().count() < MIN_PASSPHRASE {
        bail!("short_passphrase");
    }
    let servers = {
        let conn = db.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, key, created, last_seen, version, arch, host FROM servers ORDER BY created, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Server {
                id: r.get(0)?,
                name: r.get(1)?,
                key: r.get(2)?,
                created: r.get(3)?,
                last_seen: r.get(4)?,
                version: r.get(5)?,
                arch: r.get(6)?,
                host: r.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let payload = Payload {
        format: 1,
        created: now(),
        panel: crate::version().to_owned(),
        link_token: db.meta("link_token")?,
        servers,
    };
    let salt = random::<16>()?;
    let nonce = random::<12>()?;
    let key = sealing_key(&derive(passphrase, &salt)?)?;
    let mut data = serde_json::to_vec(&payload)?;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(&MAGIC[..]),
        &mut data,
    )
    .map_err(|_| anyhow!("could not seal the backup"))?;
    let mut out = Vec::with_capacity(4 + 16 + 12 + data.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&data);
    Ok(out)
}

/// Reads a backup back into `db`. A panel that already has servers refuses unless
/// `replace` is set; then its servers are swapped for the backup's. Returns how many
/// servers came back.
pub fn import(db: &Db, passphrase: &str, file: &[u8], replace: bool) -> Result<usize> {
    if file.len() < 4 + 16 + 12 + 16 || &file[..4] != MAGIC {
        bail!("not_a_backup");
    }
    let (salt, rest) = file[4..].split_at(16);
    let (nonce, sealed) = rest.split_at(12);
    let key = sealing_key(&derive(passphrase, salt)?)?;
    let mut data = sealed.to_vec();
    let plain = key
        .open_in_place(
            Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow!("bad nonce"))?,
            Aad::from(&MAGIC[..]),
            &mut data,
        )
        // A wrong passphrase and a damaged file look the same: the tag does not match.
        .map_err(|_| anyhow!("wrong_passphrase"))?;
    let payload: Payload = serde_json::from_slice(plain).context("bad_backup")?;
    if payload.format != 1 {
        bail!("bad_backup");
    }

    let mut conn = db.conn();
    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))?;
    if existing > 0 && !replace {
        bail!("not_empty");
    }
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM servers", [])?;
    for s in &payload.servers {
        tx.execute(
            "INSERT INTO servers (id, name, key, created, last_seen, version, arch, host) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![s.id, s.name, s.key, s.created, s.last_seen, s.version, s.arch, s.host],
        )?;
    }
    if let Some(token) = &payload.link_token {
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('link_token', ?1) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [token],
        )?;
    }
    tx.commit()?;
    Ok(payload.servers.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_servers() -> Db {
        let db = Db::in_memory().unwrap();
        db.set_meta("link_token", "the-link-token").unwrap();
        db.conn()
            .execute(
                "INSERT INTO servers (id, name, key, created) VALUES ('a1', 'frankfurt', 'k1', 100), ('b2', 'istanbul', 'k2', 200)",
                [],
            )
            .unwrap();
        db
    }

    #[test]
    fn a_backup_restores_the_servers_and_the_link_token_on_a_fresh_panel() {
        let file = export(&db_with_servers(), "a long enough passphrase").unwrap();
        // the file does not show what is in it
        let text = String::from_utf8_lossy(&file);
        assert!(
            !text.contains("frankfurt") && !text.contains("k1") && !text.contains("the-link-token")
        );

        let fresh = Db::in_memory().unwrap();
        fresh.set_meta("link_token", "another-token").unwrap();
        assert_eq!(
            import(&fresh, "a long enough passphrase", &file, false).unwrap(),
            2
        );
        assert_eq!(
            fresh.meta("link_token").unwrap().as_deref(),
            Some("the-link-token")
        );
        let name: String = fresh
            .conn()
            .query_row("SELECT name FROM servers WHERE id = 'b2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "istanbul");
    }

    #[test]
    fn a_wrong_passphrase_a_damaged_file_and_a_stranger_are_refused() {
        let file = export(&db_with_servers(), "a long enough passphrase").unwrap();
        let fresh = Db::in_memory().unwrap();
        let why = |r: Result<usize>| format!("{:#}", r.unwrap_err());
        assert_eq!(
            why(import(&fresh, "not the passphrase!", &file, false)),
            "wrong_passphrase"
        );
        let mut damaged = file.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert_eq!(
            why(import(&fresh, "a long enough passphrase", &damaged, false)),
            "wrong_passphrase"
        );
        assert_eq!(
            why(import(
                &fresh,
                "a long enough passphrase",
                b"hello there, this is not one at all",
                false
            )),
            "not_a_backup"
        );
        assert_eq!(
            format!("{:#}", export(&fresh, "short").unwrap_err()),
            "short_passphrase"
        );
        let count: i64 = fresh
            .conn()
            .query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a refused restore changes nothing");
    }

    #[test]
    fn a_panel_with_servers_needs_to_be_told_to_replace_them() {
        let file = export(&db_with_servers(), "a long enough passphrase").unwrap();
        let other = Db::in_memory().unwrap();
        other
            .conn()
            .execute(
                "INSERT INTO servers (id, name, key, created) VALUES ('z9', 'old', 'kz', 1)",
                [],
            )
            .unwrap();
        let why = import(&other, "a long enough passphrase", &file, false).unwrap_err();
        assert_eq!(format!("{why:#}"), "not_empty");
        assert_eq!(
            import(&other, "a long enough passphrase", &file, true).unwrap(),
            2
        );
        let ids: Vec<String> = {
            let conn = other.conn();
            let mut stmt = conn.prepare("SELECT id FROM servers ORDER BY id").unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(ids, vec!["a1", "b2"]);
    }
}
