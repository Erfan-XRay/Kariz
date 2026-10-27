//! What the panel keeps about a tunnel beyond its files on the servers: the name it is
//! shown by (any script, spaces too; the tunnel's own name stays its file and service
//! name) and where it stands in the list, as the admin arranged them.

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use rusqlite::params;
use serde::Serialize;

use crate::db::Db;

/// The longest label, in characters.
pub const MAX_LABEL: usize = 48;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Meta {
    /// The name it is shown by; none: its own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its place in the list; none: after the arranged ones, as the servers list them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
}

/// Every tunnel's label and place, by tunnel name.
pub fn all(db: &Db) -> Result<HashMap<String, Meta>> {
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT name, label, position FROM tunnel_meta")?;
    let rows = stmt.query_map([], |r| {
        let label: String = r.get(1)?;
        Ok((
            r.get::<_, String>(0)?,
            Meta {
                label: (!label.is_empty()).then_some(label),
                position: r.get(2)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `label` with its outer spaces gone and its inner runs of space made one, or `bad_label`
/// for one that is too long or holds control characters. Empty: back to the tunnel's name.
pub fn clean_label(label: &str) -> Result<String> {
    let clean = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > MAX_LABEL || clean.chars().any(char::is_control) {
        bail!("bad_label");
    }
    Ok(clean)
}

/// Shows tunnel `name` as `label` (empty: by its own name again).
pub fn set_label(db: &Db, name: &str, label: &str) -> Result<()> {
    if !crate::join::valid_name(name) {
        bail!("bad_name");
    }
    let label = clean_label(label)?;
    let conn = db.conn();
    conn.execute(
        "INSERT INTO tunnel_meta (name, label) VALUES (?1, ?2)
         ON CONFLICT (name) DO UPDATE SET label = excluded.label",
        params![name, label],
    )?;
    conn.execute(
        "DELETE FROM tunnel_meta WHERE label = '' AND position IS NULL",
        [],
    )?;
    Ok(())
}

/// Puts the tunnels in the order of `names` (each once); the ones it leaves out come after
/// them.
pub fn set_order(db: &Db, names: &[String]) -> Result<()> {
    let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
    if unique.len() != names.len() || !names.iter().all(|n| crate::join::valid_name(n)) {
        bail!("bad_order");
    }
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    tx.execute("UPDATE tunnel_meta SET position = NULL", [])?;
    for (at, name) in names.iter().enumerate() {
        tx.execute(
            "INSERT INTO tunnel_meta (name, position) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET position = excluded.position",
            params![name, at as i64],
        )?;
    }
    tx.execute(
        "DELETE FROM tunnel_meta WHERE label = '' AND position IS NULL",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// A deleted tunnel's label and place go with it.
pub fn forget(db: &Db, name: &str) -> Result<()> {
    db.conn()
        .execute("DELETE FROM tunnel_meta WHERE name = ?1", [name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_kept_cleaned_and_can_be_taken_back() {
        let db = Db::in_memory().unwrap();
        set_label(&db, "web", "  تانل   آلمان ").unwrap();
        assert_eq!(
            all(&db).unwrap()["web"].label.as_deref(),
            Some("تانل آلمان")
        );
        set_label(&db, "web", "").unwrap();
        assert!(all(&db).unwrap().is_empty());
        assert_eq!(
            set_label(&db, "web", &"x".repeat(MAX_LABEL + 1))
                .unwrap_err()
                .to_string(),
            "bad_label"
        );
        assert_eq!(
            set_label(&db, "web", "a\u{7}b").unwrap_err().to_string(),
            "bad_label"
        );
        assert_eq!(
            set_label(&db, "../web", "x").unwrap_err().to_string(),
            "bad_name"
        );
    }

    #[test]
    fn an_order_replaces_the_last_one_and_keeps_the_labels() {
        let db = Db::in_memory().unwrap();
        set_label(&db, "b", "Bee").unwrap();
        set_order(&db, &["a".into(), "b".into(), "c".into()]).unwrap();
        set_order(&db, &["c".into(), "b".into()]).unwrap();
        let meta = all(&db).unwrap();
        assert_eq!(meta["c"].position, Some(0));
        assert_eq!(meta["b"].position, Some(1));
        assert_eq!(meta["b"].label.as_deref(), Some("Bee"));
        // "a" had only its place, and lost it.
        assert!(!meta.contains_key("a"));
        assert_eq!(
            set_order(&db, &["a".into(), "a".into()])
                .unwrap_err()
                .to_string(),
            "bad_order"
        );
        forget(&db, "b").unwrap();
        assert!(!all(&db).unwrap().contains_key("b"));
    }
}
