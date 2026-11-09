//! The host firewall rules one Paqet socket needs, added when the socket opens and
//! removed when it closes.
//!
//! Paqet segments carry data without a TCP handshake, so the kernel's TCP stack sees
//! segments for a port nothing listens on. Without rules:
//!
//! - it answers each with a TCP reset, which a stateful middlebox on the path takes as
//!   the end of the flow and drops what follows;
//! - connection tracking marks the segments invalid (no handshake, sequence numbers out
//!   of its window), and a host firewall that drops invalid packets, or that accepts
//!   only established replies (ufw, firewalld), drops them before the raw socket sees
//!   them.
//!
//! So the sockets get four rules, each inserted at the top of its chain. A listener's
//! are scoped to its address and port; a dialer's to its address and the peer's address
//! and port, so every dial to one peer (each from a new local port) shares them and a
//! reconnect runs no `iptables`:
//!
//! ```text
//! raw    PREROUTING  incoming segments      CT --notrack
//! raw    OUTPUT      outgoing segments      CT --notrack
//! filter INPUT       incoming segments      ACCEPT
//! filter OUTPUT      outgoing TCP resets    DROP
//! ```
//!
//! Every rule carries the comment `kariz-paqet pid=<pid>`. A process that was killed
//! cannot remove its rules, so the first socket of a process removes the rules of
//! processes that are gone. Without `iptables` (or without the right to run it) the
//! tunnel still starts, with a warning: the rules in `docs/paqet.md` are then the
//! host's to add.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Once, Weak};

use tracing::{debug, warn};

/// The marker every Paqet rule carries, before ` pid=`.
const MARK: &str = "kariz-paqet";
const IPTABLES: &str = "iptables";
const TABLES: [&str; 2] = ["raw", "filter"];

/// One rule: its table, chain and match/target arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    table: &'static str,
    chain: &'static str,
    spec: Vec<String>,
}

/// What a set of rules is for: the local address, the local port (the listening side)
/// and the peer (the dialing side, whose own ports change with every dial).
type Scope = (Ipv4Addr, Option<u16>, Option<SocketAddrV4>);

/// The rules in place, by scope: every socket of one scope shares them, so a dialer that
/// reconnects runs no `iptables` at all.
static INSTALLED: Mutex<Vec<(Scope, Weak<Installed>)>> = Mutex::new(Vec::new());

/// A socket's hold on the rules of its scope; the last one dropped removes them.
pub(crate) struct Rules {
    _installed: Arc<Installed>,
}

struct Installed {
    scope: Scope,
    added: Vec<Rule>,
}

impl Rules {
    /// The rules of a listening socket on `local`: its port, from anyone.
    pub(crate) fn listener(local: SocketAddrV4) -> Self {
        Self::install((*local.ip(), Some(local.port()), None))
    }

    /// The rules of a dialing socket on `local` that talks only to `peer`, whatever local
    /// port it was given.
    pub(crate) fn dialer(local: Ipv4Addr, peer: SocketAddrV4) -> Self {
        Self::install((local, None, Some(peer)))
    }

    fn install(scope: Scope) -> Self {
        static CLEANUP: Once = Once::new();
        CLEANUP.call_once(remove_stale);

        // Held while the rules are added, so two sockets of one scope never both add them.
        let mut installed = lock(&INSTALLED);
        installed.retain(|(_, weak)| weak.strong_count() > 0);
        if let Some(rules) = installed
            .iter()
            .find(|(s, _)| *s == scope)
            .and_then(|(_, weak)| weak.upgrade())
        {
            return Self { _installed: rules };
        }
        let rules = Arc::new(Installed {
            scope,
            added: add_rules(scope),
        });
        installed.push((scope, Arc::downgrade(&rules)));
        Self { _installed: rules }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn add_rules(scope: Scope) -> Vec<Rule> {
    let (local, port, peer) = scope;
    let comment = format!("{MARK} pid={}", std::process::id());
    let mut added = Vec::new();

    for rule in rules_for(local, port, peer, &comment) {
        match run(&rule_args("-I", &rule)) {
            Ok(()) => added.push(rule),
            Err(e) => {
                warn!(
                    error = %e,
                    table = rule.table,
                    chain = rule.chain,
                    "paqet could not add a firewall rule; add the rules in docs/paqet.md \
                     yourself if the tunnel does not connect"
                );
                // Without iptables every rule fails alike: say it once.
                if e.kind() == std::io::ErrorKind::NotFound {
                    break;
                }
            }
        }
    }

    if !added.is_empty() {
        debug!(%local, ?port, ?peer, rules = added.len(), "paqet firewall rules added");
    }
    added
}

impl Drop for Installed {
    fn drop(&mut self) {
        // Under the lock: a socket of this scope that opens meanwhile adds the same rules
        // again, and `-D` removes one copy of each, so its copy stays.
        let mut installed = lock(&INSTALLED);
        for rule in self.added.iter().rev() {
            if let Err(e) = run(&rule_args("-D", rule)) {
                debug!(error = %e, chain = rule.chain, "could not remove a paqet firewall rule");
            }
        }
        installed.retain(|(scope, weak)| *scope != self.scope || weak.strong_count() > 0);
    }
}

/// The four rules of a socket on `local` (see the module documentation): its `port` on
/// the listening side, the `peer`'s address and port on the dialing side.
pub(crate) fn rules_for(
    local: Ipv4Addr,
    port: Option<u16>,
    peer: Option<SocketAddrV4>,
    comment: &str,
) -> Vec<Rule> {
    // Incoming: from the peer (when known) to this socket. Outgoing: the reverse.
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    if let Some(peer) = peer {
        incoming.extend(["-s".into(), peer.ip().to_string()]);
    }
    incoming.extend(["-d".into(), local.to_string(), "-p".into(), "tcp".into()]);
    outgoing.extend(["-s".into(), local.to_string()]);
    if let Some(peer) = peer {
        outgoing.extend(["-d".into(), peer.ip().to_string()]);
    }
    outgoing.extend(["-p".into(), "tcp".into()]);
    if let Some(peer) = peer {
        incoming.extend(["--sport".into(), peer.port().to_string()]);
        outgoing.extend(["--dport".into(), peer.port().to_string()]);
    }
    if let Some(port) = port {
        incoming.extend(["--dport".into(), port.to_string()]);
        outgoing.extend(["--sport".into(), port.to_string()]);
    }

    // Matches, then the comment, then the target: the order `iptables -S` prints.
    let with = |base: &[String], matches: &[&str], target: &[&str]| -> Vec<String> {
        let mut spec = base.to_vec();
        spec.extend(matches.iter().map(|s| (*s).to_string()));
        spec.extend([
            "-m".into(),
            "comment".into(),
            "--comment".into(),
            comment.to_string(),
        ]);
        spec.extend(target.iter().map(|s| (*s).to_string()));
        spec
    };
    let notrack = ["-j", "CT", "--notrack"];
    vec![
        Rule {
            table: "raw",
            chain: "PREROUTING",
            spec: with(&incoming, &[], &notrack),
        },
        Rule {
            table: "raw",
            chain: "OUTPUT",
            spec: with(&outgoing, &[], &notrack),
        },
        Rule {
            table: "filter",
            chain: "INPUT",
            spec: with(&incoming, &[], &["-j", "ACCEPT"]),
        },
        Rule {
            table: "filter",
            chain: "OUTPUT",
            spec: with(&outgoing, &["--tcp-flags", "RST", "RST"], &["-j", "DROP"]),
        },
    ]
}

/// `iptables -w 5 -t <table> <action> <chain> [1] <spec...>`.
fn rule_args(action: &str, rule: &Rule) -> Vec<String> {
    let mut args = vec![
        "-w".into(),
        "5".into(),
        "-t".into(),
        rule.table.into(),
        action.into(),
        rule.chain.into(),
    ];
    if action == "-I" {
        args.push("1".into());
    }
    args.extend(rule.spec.iter().cloned());
    args
}

fn run(args: &[String]) -> std::io::Result<()> {
    let out = Command::new(IPTABLES)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stderr);
    Err(std::io::Error::other(format!(
        "iptables {}: {}",
        args.join(" "),
        text.trim()
    )))
}

/// Removes the Paqet rules of processes that are no longer running.
fn remove_stale() {
    for table in TABLES {
        let listing = match Command::new(IPTABLES)
            .args(["-w", "5", "-t", table, "-S"])
            .stdin(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
            Ok(_) | Err(_) => continue,
        };
        for args in stale_deletions(&listing, table, |pid| {
            Path::new(&format!("/proc/{pid}")).exists()
        }) {
            if let Err(e) = run(&args) {
                debug!(error = %e, "could not remove a stale paqet firewall rule");
            }
        }
    }
}

/// The `iptables` arguments that delete each Paqet rule in an `iptables -S` listing whose
/// process is gone (`alive` says whether a pid still runs).
pub(crate) fn stale_deletions(
    listing: &str,
    table: &str,
    alive: impl Fn(u32) -> bool,
) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in listing.lines() {
        let words = split_words(line);
        if words.first().map(String::as_str) != Some("-A") || words.len() < 2 {
            continue;
        }
        let Some(pid) = words.iter().find_map(|w| {
            w.strip_prefix(MARK)?
                .strip_prefix(" pid=")?
                .parse::<u32>()
                .ok()
        }) else {
            continue;
        };
        if pid == std::process::id() || alive(pid) {
            continue;
        }
        let mut args = vec![
            "-w".to_string(),
            "5".into(),
            "-t".into(),
            table.into(),
            "-D".into(),
        ];
        args.extend(words.into_iter().skip(1));
        out.push(args);
    }
    out
}

/// Splits an `iptables -S` line into words: spaces separate them, double quotes group
/// them (and `\"` is a quote inside a group).
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut any = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            c => word.push(c),
        }
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(a: u8, port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, a), port)
    }

    fn joined(rule: &Rule) -> String {
        format!("{} {} {}", rule.table, rule.chain, rule.spec.join(" "))
    }

    #[test]
    fn a_listener_accepts_its_port_from_anyone_and_drops_its_resets() {
        let rules: Vec<String> =
            rules_for(*addr(21, 0).ip(), Some(31080), None, "kariz-paqet pid=7")
                .iter()
                .map(joined)
                .collect();
        assert_eq!(
            rules,
            [
                "raw PREROUTING -d 192.0.2.21 -p tcp --dport 31080 -m comment --comment kariz-paqet pid=7 -j CT --notrack",
                "raw OUTPUT -s 192.0.2.21 -p tcp --sport 31080 -m comment --comment kariz-paqet pid=7 -j CT --notrack",
                "filter INPUT -d 192.0.2.21 -p tcp --dport 31080 -m comment --comment kariz-paqet pid=7 -j ACCEPT",
                "filter OUTPUT -s 192.0.2.21 -p tcp --sport 31080 --tcp-flags RST RST -m comment --comment kariz-paqet pid=7 -j DROP",
            ]
        );
    }

    #[test]
    fn a_dialer_is_scoped_to_its_peer_whatever_its_own_port() {
        let rules: Vec<String> = rules_for(*addr(22, 0).ip(), None, Some(addr(21, 31080)), "c")
            .iter()
            .map(joined)
            .collect();
        assert_eq!(
            rules[0],
            "raw PREROUTING -s 192.0.2.21 -d 192.0.2.22 -p tcp --sport 31080 -m comment --comment c -j CT --notrack"
        );
        assert_eq!(
            rules[3],
            "filter OUTPUT -s 192.0.2.22 -d 192.0.2.21 -p tcp --dport 31080 --tcp-flags RST RST -m comment --comment c -j DROP"
        );
    }

    #[test]
    fn insert_goes_to_the_top_and_delete_names_the_same_rule() {
        let rule = &rules_for(*addr(21, 0).ip(), Some(1), None, "c")[2];
        let insert = rule_args("-I", rule);
        assert_eq!(
            &insert[..7],
            ["-w", "5", "-t", "filter", "-I", "INPUT", "1"]
        );
        let delete = rule_args("-D", rule);
        assert_eq!(&delete[4..6], ["-D", "INPUT"]);
        assert_eq!(delete[6..], insert[7..]);
    }

    #[test]
    fn only_rules_of_dead_processes_are_removed() {
        let listing = "-P INPUT ACCEPT\n\
            -A INPUT -d 192.0.2.21/32 -p tcp -m tcp --dport 31080 -m comment --comment \"kariz-paqet pid=100\" -j ACCEPT\n\
            -A INPUT -d 192.0.2.21/32 -p tcp -m tcp --dport 31081 -m comment --comment \"kariz-paqet pid=200\" -j ACCEPT\n\
            -A INPUT -p tcp --dport 22 -m comment --comment \"ssh\" -j ACCEPT\n";
        let gone = stale_deletions(listing, "filter", |pid| pid == 200);
        assert_eq!(gone.len(), 1);
        assert_eq!(
            gone[0].join(" "),
            "-w 5 -t filter -D INPUT -d 192.0.2.21/32 -p tcp -m tcp --dport 31080 -m comment --comment kariz-paqet pid=100 -j ACCEPT"
        );
    }

    #[test]
    fn words_keep_quoted_spaces() {
        assert_eq!(
            split_words(r#"-A X --comment "a b \"c\"" -j DROP"#),
            ["-A", "X", "--comment", "a b \"c\"", "-j", "DROP"]
        );
        assert_eq!(split_words(r#"--comment """#), ["--comment", ""]);
    }
}
