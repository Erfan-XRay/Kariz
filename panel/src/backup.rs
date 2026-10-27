//! A backup of the panel: the servers it knows (with the keys they prove themselves with),
//! the link token they dial with, and where the panel dials the ones it connects to
//! (reverse), in one file locked with a passphrase. Since 2.7.0-beta.4 also the private
//! networks and their links, the servers' addresses, automatic restarts, usage quotas,
//! the tunnels' labels and order, and the Telegram and update settings ([`TABLES`],
//! [`META_KEYS`]).
//!
//! Tunnels are not in it: they live on the servers, in their own files, and the panel
//! reads them from there. Sessions, login links, the admin password, history and usage
//! counts are not in it either.
//!
//! The file is `KZB1`, a random salt, a random nonce, then the JSON sealed with
//! ChaCha20-Poly1305 under a key that Argon2id makes from the passphrase.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::now;
use crate::db::Db;

const MAGIC: &[u8; 4] = b"KZB1";
pub const MIN_PASSPHRASE: usize = 10;

/// Tables a backup carries whole, a table before the ones that refer to it.
const TABLES: &[&str] = &[
    "networks",
    "net_links",
    "pending_net_downs",
    "server_addrs",
    "server_addr6",
    "server_seen",
    "schedules",
    "usage_quotas",
    "tunnel_meta",
];

/// Settings a backup carries (the bot, its quiet hours and digest, automatic updates).
const META_KEYS: &[&str] = &[
    "telegram",
    "telegram_mute",
    "telegram_digest",
    "update_channel",
    "update_auto",
];

type Row = serde_json::Map<String, Value>;

/// Every row of `table`, as column name to value.
fn dump(conn: &Connection, table: &str) -> Result<Vec<Row>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table}"))?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let rows = stmt.query_map([], |r| {
        let mut row = Row::new();
        for (i, name) in names.iter().enumerate() {
            let value = match r.get_ref(i)? {
                ValueRef::Null | ValueRef::Blob(_) => Value::Null,
                ValueRef::Integer(n) => n.into(),
                ValueRef::Real(f) => f.into(),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
            };
            row.insert(name.clone(), value);
        }
        Ok(row)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Puts `rows` into `table` (emptied first by the caller): the columns this panel's table
/// has, so a backup from a panel a version apart still restores.
fn load(conn: &Connection, table: &str, rows: &[Row]) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    for row in rows {
        let (names, values): (Vec<&String>, Vec<SqlValue>) = columns
            .iter()
            .filter_map(|c| {
                let v = row.get(c)?;
                Some((
                    c,
                    match v {
                        Value::Null => SqlValue::Null,
                        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
                        Value::Number(n) => match n.as_i64() {
                            Some(i) => SqlValue::Integer(i),
                            None => SqlValue::Real(n.as_f64().unwrap_or_default()),
                        },
                        Value::String(s) => SqlValue::Text(s.clone()),
                        other => SqlValue::Text(other.to_string()),
                    },
                ))
            })
            .unzip();
        if names.is_empty() {
            continue;
        }
        let marks = (1..=names.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let names = names
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute(
            &format!("INSERT INTO {table} ({names}) VALUES ({marks})"),
            params_from_iter(values),
        )?;
    }
    Ok(())
}

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
    /// Not in backups from before servers were arranged: then they keep the order of the
    /// list (oldest first).
    #[serde(default)]
    position: Option<i64>,
}

/// A server the panel connects to (reverse): where it dials it.
#[derive(Debug, Serialize, Deserialize)]
struct ReverseRow {
    server: String,
    host: String,
    port: u16,
    transport: Option<String>,
}

/// A server with an agents port of its own: the port its agent dials.
#[derive(Debug, Serialize, Deserialize)]
struct PortRow {
    server: String,
    port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    format: u32,
    created: i64,
    panel: String,
    link_token: Option<String>,
    servers: Vec<Server>,
    /// Not in backups from before reverse links: then there are none.
    #[serde(default)]
    reverse: Vec<ReverseRow>,
    /// Not in backups from before agents ports of a server's own: then there are none.
    #[serde(default)]
    ports: Vec<PortRow>,
    /// Where the panel's own server stood among them; none: before them all.
    #[serde(default)]
    local_position: Option<i64>,
    /// The rows of [`TABLES`]. None in backups from before 2.7.0-beta.4: then the panel's
    /// own are left as they are.
    #[serde(default)]
    tables: Option<BTreeMap<String, Vec<Row>>>,
    /// The settings of [`META_KEYS`] that were set.
    #[serde(default)]
    meta: BTreeMap<String, String>,
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
            "SELECT id, name, key, created, last_seen, version, arch, host, position FROM servers
             ORDER BY position, created, id",
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
                position: r.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let reverse = {
        let conn = db.conn();
        let mut stmt = conn
            .prepare("SELECT server, host, port, transport FROM reverse_links ORDER BY server")?;
        let rows = stmt.query_map([], |r| {
            Ok(ReverseRow {
                server: r.get(0)?,
                host: r.get(1)?,
                port: r.get(2)?,
                transport: r.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let ports = {
        let conn = db.conn();
        let mut stmt = conn.prepare("SELECT server, port FROM agent_ports ORDER BY server")?;
        let rows = stmt.query_map([], |r| {
            Ok(PortRow {
                server: r.get(0)?,
                port: r.get(1)?,
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
        reverse,
        ports,
        local_position: db.meta("local_position")?.and_then(|p| p.parse().ok()),
        tables: Some({
            let conn = db.conn();
            TABLES
                .iter()
                .map(|t| Ok((t.to_string(), dump(&conn, t)?)))
                .collect::<Result<_>>()?
        }),
        meta: {
            let mut meta = BTreeMap::new();
            for key in META_KEYS {
                if let Some(value) = db.meta(key)? {
                    meta.insert(key.to_string(), value);
                }
            }
            meta
        },
    };
    seal(serde_json::to_vec(&payload)?, passphrase)
}

/// `data` locked with `passphrase`, in the backup file's form.
fn seal(mut data: Vec<u8>, passphrase: &str) -> Result<Vec<u8>> {
    let salt = random::<16>()?;
    let nonce = random::<12>()?;
    let key = sealing_key(&derive(passphrase, &salt)?)?;
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
    for (at, s) in payload.servers.iter().enumerate() {
        tx.execute(
            "INSERT INTO servers (id, name, key, created, last_seen, version, arch, host, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                s.id,
                s.name,
                s.key,
                s.created,
                s.last_seen,
                s.version,
                s.arch,
                s.host,
                s.position.unwrap_or(at as i64)
            ],
        )?;
    }
    match payload.local_position {
        Some(p) => tx.execute(
            "INSERT INTO meta (key, value) VALUES ('local_position', ?1) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [p.to_string()],
        )?,
        None => tx.execute("DELETE FROM meta WHERE key = 'local_position'", [])?,
    };
    tx.execute("DELETE FROM reverse_links", [])?;
    for r in &payload.reverse {
        tx.execute(
            "INSERT INTO reverse_links (server, host, port, transport) VALUES (?1, ?2, ?3, ?4)",
            params![r.server, r.host, r.port, r.transport],
        )?;
    }
    tx.execute("DELETE FROM agent_ports", [])?;
    for p in &payload.ports {
        tx.execute(
            "INSERT INTO agent_ports (server, port) VALUES (?1, ?2)",
            params![p.server, p.port],
        )?;
    }
    if let Some(token) = &payload.link_token {
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('link_token', ?1) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [token],
        )?;
    }
    if let Some(tables) = &payload.tables {
        // Emptied the other way round, so no row is left pointing at a deleted one.
        for table in TABLES.iter().rev() {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        for table in TABLES {
            load(
                &tx,
                table,
                tables.get(*table).map_or(&[][..], Vec::as_slice),
            )?;
        }
        for key in META_KEYS {
            match payload.meta.get(*key) {
                Some(value) => tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?,
                None => tx.execute("DELETE FROM meta WHERE key = ?1", [key])?,
            };
        }
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
                "INSERT INTO servers (id, name, key, created) VALUES ('a1', 'frankfurt', 'key-of-frankfurt-7f3a9c2e', 100), ('b2', 'istanbul', 'key-of-istanbul-5b1d8e04', 200)",
                [],
            )
            .unwrap();
        // The panel connects to istanbul (reverse).
        db.conn()
            .execute(
                "INSERT INTO reverse_links (server, host, port, transport) VALUES ('b2', 'ist.example.com', 29001, NULL)",
                [],
            )
            .unwrap();
        // frankfurt's agent dials a port of its own.
        db.conn()
            .execute(
                "INSERT INTO agent_ports (server, port) VALUES ('a1', 8443)",
                [],
            )
            .unwrap();
        db
    }

    #[test]
    fn a_backup_brings_back_networks_settings_and_tunnel_labels() {
        let db = db_with_servers();
        db.conn()
            .execute_batch(
                "INSERT INTO networks (id, name, cidr, created, kind, port, ipv6) VALUES ('n1', 'core', '10.77.0.0/16', 5, 'gre', 0, 0);
                 INSERT INTO net_links (id, network, a, b, subnet, addr_a, addr_b, gre_key, ifname, created, mtu, mtu_how, ipv6)
                     VALUES ('l1', 'n1', 'a1', 'b2', '10.77.0.0/30', '10.77.0.1', '10.77.0.2', 7, 'kz-l1', 6, 1400, 'measured', 0);
                 INSERT INTO server_addrs (server, addr) VALUES ('a1', 'fra.example.com');
                 INSERT INTO schedules (kind, subject, mode, every_secs, since) VALUES ('tunnel', 'web', 'every', 21600, 9);
                 INSERT INTO usage_quotas (server, limit_bytes, count, reset_day, warn) VALUES ('a1', 1000, 'both', 1, 80);",
            )
            .unwrap();
        db.set_meta("telegram", "{\"token\":\"bot-secret-token-1234\"}")
            .unwrap();
        crate::tunnel_meta::set_label(&db, "web", "وب آلمان").unwrap();
        crate::tunnel_meta::set_order(&db, &["web".into()]).unwrap();
        let file = export(&db, "a long enough passphrase").unwrap();
        assert!(!String::from_utf8_lossy(&file).contains("bot-secret-token-1234"));

        let fresh = Db::in_memory().unwrap();
        // A setting the backup does not have goes, like the network the panel had.
        fresh.set_meta("update_auto", "1").unwrap();
        fresh
            .conn()
            .execute(
                "INSERT INTO networks (id, name, cidr, created) VALUES ('n9', 'old', '10.9.0.0/16', 1)",
                [],
            )
            .unwrap();
        import(&fresh, "a long enough passphrase", &file, false).unwrap();
        let conn = fresh.conn();
        let networks: Vec<String> = conn
            .prepare("SELECT name FROM networks")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(networks, ["core"]);
        let link: (String, i64, String) = conn
            .query_row(
                "SELECT ifname, mtu, mtu_how FROM net_links WHERE id = 'l1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(link, ("kz-l1".into(), 1400, "measured".into()));
        let addr: String = conn
            .query_row(
                "SELECT addr FROM server_addrs WHERE server = 'a1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(addr, "fra.example.com");
        let every: i64 = conn
            .query_row(
                "SELECT every_secs FROM schedules WHERE subject = 'web'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(every, 21600);
        drop(conn);
        assert_eq!(
            fresh.meta("telegram").unwrap().as_deref(),
            Some("{\"token\":\"bot-secret-token-1234\"}")
        );
        assert_eq!(fresh.meta("update_auto").unwrap(), None);
        let meta = crate::tunnel_meta::all(&fresh).unwrap();
        assert_eq!(meta["web"].label.as_deref(), Some("وب آلمان"));
        assert_eq!(meta["web"].position, Some(0));
    }

    #[test]
    fn a_backup_restores_the_servers_and_the_link_token_on_a_fresh_panel() {
        let file = export(&db_with_servers(), "a long enough passphrase").unwrap();
        // the file does not show what is in it (long enough strings that random bytes never
        // spell them by chance; a two-letter key once did)
        let text = String::from_utf8_lossy(&file);
        assert!(
            !text.contains("frankfurt")
                && !text.contains("key-of-frankfurt-7f3a9c2e")
                && !text.contains("the-link-token")
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
        // And where the panel dials the one it connects to.
        let at: (String, u16) = fresh
            .conn()
            .query_row(
                "SELECT host, port FROM reverse_links WHERE server = 'b2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(at, ("ist.example.com".to_owned(), 29001));
        // And the agents port of its own one of them dials.
        let own: u16 = fresh
            .conn()
            .query_row(
                "SELECT port FROM agent_ports WHERE server = 'a1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(own, 8443);
    }

    fn order(db: &Db) -> Vec<String> {
        let conn = db.conn();
        let mut stmt = conn
            .prepare("SELECT id FROM servers ORDER BY position, created, id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn a_backup_keeps_the_order_the_servers_were_arranged_in() {
        let db = db_with_servers();
        // istanbul first, then the panel's own, then frankfurt.
        db.conn()
            .execute_batch(
                "UPDATE servers SET position = 2 WHERE id = 'a1';
                 UPDATE servers SET position = 0 WHERE id = 'b2';",
            )
            .unwrap();
        db.set_meta("local_position", "1").unwrap();
        let file = export(&db, "a long enough passphrase").unwrap();

        let fresh = Db::in_memory().unwrap();
        import(&fresh, "a long enough passphrase", &file, false).unwrap();
        assert_eq!(order(&fresh), ["b2", "a1"]);
        assert_eq!(fresh.meta("local_position").unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn a_backup_from_before_arranging_keeps_its_list_order() {
        // As an older panel wrote it: no positions, the servers oldest first.
        let old = serde_json::json!({
            "format": 1, "created": 1, "panel": "2.5.1", "link_token": "t",
            "servers": [
                {"id": "z9", "name": "older", "key": "k1", "created": 50, "last_seen": 0,
                 "version": "", "arch": "", "host": ""},
                {"id": "a1", "name": "newer", "key": "k2", "created": 90, "last_seen": 0,
                 "version": "", "arch": "", "host": ""}
            ]
        });
        let file = seal(old.to_string().into_bytes(), "a long enough passphrase").unwrap();
        let fresh = Db::in_memory().unwrap();
        fresh.set_meta("local_position", "5").unwrap();
        assert_eq!(
            import(&fresh, "a long enough passphrase", &file, false).unwrap(),
            2
        );
        assert_eq!(order(&fresh), ["z9", "a1"]);
        // And the panel's own goes back to the top.
        assert_eq!(fresh.meta("local_position").unwrap(), None);
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
