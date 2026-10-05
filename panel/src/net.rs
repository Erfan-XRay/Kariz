//! What an agent does for a private GRE network: make the
//! `kz-` interface of a link, address it, take it down, test the path across it, and keep
//! the list so the interfaces come back after a reboot.
//!
//! A request carries **data**, never a command: every field is parsed and range-checked
//! first, the programs that run (`ip`, `ping`) get fixed arguments with the checked values,
//! and only interfaces whose names start with `kz-` are ever touched.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::manage::Fut;
use crate::wire::{NetIface, NetSpec, NetStatus, PingReply};

/// GRE adds 24 bytes to every packet on a 1500-byte path.
pub const DEFAULT_MTU: u32 = 1476;

/// `kz-` and one to eight small letters or digits.
pub fn valid_ifname(name: &str) -> bool {
    name.strip_prefix("kz-").is_some_and(|rest| {
        (1..=8).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Checks every field of a link request; the parsed addresses come back.
pub fn validate(spec: &NetSpec) -> Result<(Ipv4Addr, Ipv4Addr, Ipv4Addr, Ipv4Addr)> {
    if !valid_ifname(&spec.name) {
        bail!("the interface name must be kz- and up to eight small letters or digits");
    }
    let ip = |s: &str, what: &str| -> Result<Ipv4Addr> {
        s.parse::<IpAddr>()
            .ok()
            .and_then(|a| match a {
                IpAddr::V4(v4) => Some(v4),
                IpAddr::V6(_) => None,
            })
            .with_context(|| format!("{what} is not an IPv4 address"))
    };
    let local = ip(&spec.local, "local")?;
    let remote = ip(&spec.remote, "remote")?;
    let address = ip(&spec.address, "address")?;
    let peer = ip(&spec.peer, "peer")?;
    for (a, what) in [(local, "local"), (remote, "remote")] {
        if a.is_unspecified() || a.is_loopback() || a.is_multicast() || a.is_broadcast() {
            bail!("{what} must be a unicast address of a server");
        }
    }
    if local == remote {
        bail!("local and remote are the same address");
    }
    if !(30..=31).contains(&spec.prefix) {
        bail!("prefix must be 30 or 31");
    }
    if spec.key == 0 {
        bail!("the GRE key must be 1 or more");
    }
    if !(576..=DEFAULT_MTU).contains(&spec.mtu) {
        bail!("mtu must be between 576 and {DEFAULT_MTU}");
    }
    let mask = u32::MAX << (32 - u32::from(spec.prefix));
    if u32::from(address) & mask != u32::from(peer) & mask || address == peer {
        bail!(
            "address and peer must be two different hosts of one /{}",
            spec.prefix
        );
    }
    for a in [address, peer] {
        if a.is_unspecified() || a.is_loopback() || a.is_multicast() {
            bail!("the link addresses must be private unicast addresses");
        }
    }
    Ok((local, remote, address, peer))
}

/// The `ip` invocations that make the link, in order. Any interface of that name is
/// removed first, so the same request twice (or with new addresses) is fine.
pub fn commands(spec: &NetSpec) -> Result<Vec<Vec<String>>> {
    let (local, remote, address, _) = validate(spec)?;
    let s = |x: &str| x.to_owned();
    Ok(vec![
        vec![
            s("tunnel"),
            s("add"),
            spec.name.clone(),
            s("mode"),
            s("gre"),
            s("local"),
            local.to_string(),
            s("remote"),
            remote.to_string(),
            s("key"),
            spec.key.to_string(),
            s("ttl"),
            s("64"),
        ],
        vec![
            s("addr"),
            s("add"),
            format!("{address}/{}", spec.prefix),
            s("dev"),
            spec.name.clone(),
        ],
        vec![
            s("link"),
            s("set"),
            spec.name.clone(),
            s("mtu"),
            spec.mtu.to_string(),
            s("up"),
        ],
    ])
}

/// The average round trip in `ping`'s summary line (`rtt min/avg/max/mdev = 0.4/0.6/...`).
pub fn parse_ping(output: &str) -> Option<f64> {
    let line = output.lines().find(|l| l.contains("min/avg/max"))?;
    line.split('=')
        .nth(1)?
        .trim()
        .split('/')
        .nth(1)?
        .parse()
        .ok()
}

/// How the agent runs programs; the tests record instead.
pub trait Exec: Send + Sync {
    fn run(&self, program: &str, args: Vec<String>) -> Fut<Result<(bool, String)>>;
}

pub struct Real;

impl Exec for Real {
    fn run(&self, program: &str, args: Vec<String>) -> Fut<Result<(bool, String)>> {
        let program = program.to_owned();
        Box::pin(async move { crate::manage::run(&program, &args, Duration::from_secs(15)).await })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    link: Vec<NetSpec>,
}

/// The private-network side of an agent.
pub struct Net {
    exec: Arc<dyn Exec>,
    /// Where the links are kept between runs; `None` keeps them in memory only.
    state: Option<PathBuf>,
    lock: tokio::sync::Mutex<()>,
    /// Where the kernel lists the interfaces (`/sys/class/net`).
    sys: PathBuf,
}

impl Net {
    pub fn new(state: Option<PathBuf>) -> Self {
        Self::with_exec(state, Arc::new(Real))
    }

    pub fn with_exec(state: Option<PathBuf>, exec: Arc<dyn Exec>) -> Self {
        Self {
            exec,
            state,
            lock: tokio::sync::Mutex::new(()),
            sys: PathBuf::from("/sys/class/net"),
        }
    }

    /// Like [`Net::with_exec`], with the interfaces looked for in `sys` (for the tests).
    pub fn with_sys(mut self, sys: PathBuf) -> Self {
        self.sys = sys;
        self
    }

    /// Whether the kernel has an interface of that name.
    fn exists(&self, name: &str) -> bool {
        self.sys.join(name).exists()
    }

    fn load(&self) -> State {
        self.state
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, state: &State) -> Result<()> {
        if let Some(path) = &self.state {
            let text = format!(
                "# The private network links of this server (kariz-panel). The agent makes them\n# again when it starts.\n{}",
                toml::to_string_pretty(state)?
            );
            crate::agent::write_private(path, text.as_bytes())?;
        }
        Ok(())
    }

    async fn ip(&self, args: Vec<String>) -> Result<String> {
        let (ok, text) = self.exec.run("ip", args.clone()).await?;
        if ok {
            Ok(text)
        } else {
            bail!("ip {} failed: {}", args.join(" "), text.trim())
        }
    }

    /// Makes the interface (again, if it exists) without touching the saved list.
    async fn apply(&self, spec: &NetSpec) -> Result<()> {
        let cmds = commands(spec)?;
        // A link of that name that is already there is replaced.
        let _ = self
            .exec
            .run("ip", vec!["link".into(), "del".into(), spec.name.clone()])
            .await;
        for cmd in cmds {
            if let Err(e) = self.ip(cmd).await {
                let _ = self
                    .exec
                    .run("ip", vec!["link".into(), "del".into(), spec.name.clone()])
                    .await;
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn up(&self, spec: NetSpec) -> Result<()> {
        let _guard = self.lock.lock().await;
        self.apply(&spec).await?;
        let mut state = self.load();
        state.link.retain(|l| l.name != spec.name);
        state.link.push(spec);
        self.save(&state)
    }

    /// Adds a link to the saved list without making it (a join code's link: the agent makes
    /// it when it starts, before it dials). True when the list changed: the link is new, or
    /// it was saved with other settings.
    pub fn keep(&self, spec: NetSpec) -> Result<bool> {
        validate(&spec)?;
        let mut state = self.load();
        if state.link.contains(&spec) {
            return Ok(false);
        }
        state.link.retain(|l| l.name != spec.name);
        state.link.push(spec);
        self.save(&state)?;
        Ok(true)
    }

    pub async fn down(&self, name: &str) -> Result<()> {
        if !valid_ifname(name) {
            bail!("the interface name must be kz- and up to eight small letters or digits");
        }
        let _guard = self.lock.lock().await;
        let mut state = self.load();
        state.link.retain(|l| l.name != name);
        self.save(&state)?;
        // Already gone is fine.
        let _ = self
            .exec
            .run("ip", vec!["link".into(), "del".into(), name.to_owned()])
            .await;
        Ok(())
    }

    /// Makes this list the server's links: the ones not in it are removed, the missing or
    /// changed ones are made, and the ones that are already right are left alone (so
    /// reconnecting an agent does not drop a working link).
    pub async fn sync(&self, wanted: Vec<NetSpec>) -> Result<()> {
        let _guard = self.lock.lock().await;
        let state = self.load();
        for old in &state.link {
            if !wanted.iter().any(|w| w.name == old.name) {
                let _ = self
                    .exec
                    .run("ip", vec!["link".into(), "del".into(), old.name.clone()])
                    .await;
            }
        }
        let mut failed = Vec::new();
        for spec in &wanted {
            let same = state.link.iter().any(|l| l == spec);
            if same && self.exists(&spec.name) {
                continue;
            }
            if let Err(e) = self.apply(spec).await {
                failed.push(format!("{}: {e:#}", spec.name));
            }
        }
        self.save(&State { link: wanted })?;
        if failed.is_empty() {
            Ok(())
        } else {
            bail!("{}", failed.join("; "))
        }
    }

    /// Makes every saved link again (the agent's start, after a reboot). Returns how many
    /// came up; the ones that did not are logged by the caller through the errors.
    ///
    /// An interface the kernel already has (the agent was only restarted, for an update) is
    /// left as it is and only set up: making it again would cut the traffic of every tunnel
    /// that runs over it for no reason.
    pub async fn restore(&self) -> Vec<(String, Result<()>)> {
        let _guard = self.lock.lock().await;
        let mut out = Vec::new();
        for spec in self.load().link {
            let name = spec.name.clone();
            let made = if self.exists(&name) && validate(&spec).is_ok() {
                self.ip(vec!["link".into(), "set".into(), name.clone(), "up".into()])
                    .await
                    .map(|_| ())
            } else {
                self.apply(&spec).await
            };
            out.push((name, made));
        }
        out
    }

    /// Pings the far end of a link across it.
    pub async fn ping(&self, name: &str) -> PingReply {
        let fail = |e: String| PingReply {
            ok: false,
            rtt_ms: None,
            error: Some(e),
        };
        let Some(spec) = self.load().link.into_iter().find(|l| l.name == name) else {
            return fail("no such link".into());
        };
        if validate(&spec).is_err() {
            return fail("the saved link is not valid".into());
        }
        let args = ["-c", "3", "-W", "2", "-I", name, spec.peer.as_str()]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        match self.exec.run("ping", args).await {
            Ok((true, text)) => PingReply {
                ok: true,
                rtt_ms: parse_ping(&text),
                error: None,
            },
            Ok((false, _)) => fail("no answer across the link".into()),
            Err(e) => fail(format!("{e:#}")),
        }
    }

    /// Whether this server can make GRE interfaces, and the state of the links it keeps.
    pub async fn status(&self) -> NetStatus {
        let probe = "kz-probe";
        let _ = self
            .exec
            .run("ip", vec!["link".into(), "del".into(), probe.into()])
            .await;
        let made = self
            .exec
            .run(
                "ip",
                [
                    "tunnel",
                    "add",
                    probe,
                    "mode",
                    "gre",
                    "local",
                    "192.0.2.1",
                    "remote",
                    "192.0.2.2",
                ]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            )
            .await;
        let (gre, reason) = match made {
            Ok((true, _)) => (true, None),
            Ok((false, text)) => (false, Some(text.trim().to_owned())),
            Err(e) => (false, Some(format!("{e:#}"))),
        };
        if gre {
            let _ = self
                .exec
                .run("ip", vec!["link".into(), "del".into(), probe.into()])
                .await;
        }
        let links = self
            .load()
            .link
            .into_iter()
            .map(|l| {
                let base = format!("/sys/class/net/{}", l.name);
                let exists = std::path::Path::new(&base).exists();
                let up = std::fs::read_to_string(format!("{base}/operstate"))
                    .map(|s| s.trim() != "down")
                    .unwrap_or(false);
                NetIface {
                    name: l.name,
                    address: l.address,
                    exists,
                    up: exists && up,
                }
            })
            .collect();
        NetStatus { gre, reason, links }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn spec() -> NetSpec {
        NetSpec {
            name: "kz-a1b2c".into(),
            local: "203.0.113.5".into(),
            remote: "198.51.100.7".into(),
            key: 4242,
            address: "10.77.0.1".into(),
            peer: "10.77.0.2".into(),
            prefix: 30,
            mtu: 1476,
        }
    }

    /// Records what would run, and answers as told.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail_on: Option<&'static str>,
        ping: &'static str,
    }

    impl Exec for Fake {
        fn run(&self, program: &str, args: Vec<String>) -> Fut<Result<(bool, String)>> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            let fail = self.fail_on.is_some_and(|f| line.contains(f));
            let out = if program == "ping" {
                self.ping.to_owned()
            } else {
                String::new()
            };
            Box::pin(async move { Ok((!fail, out)) })
        }
    }

    fn calls(fake: &Fake) -> Vec<String> {
        fake.calls.lock().unwrap().clone()
    }

    #[test]
    fn a_link_makes_three_fixed_ip_commands_from_checked_values() {
        let cmds = commands(&spec()).unwrap();
        let lines: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
        assert_eq!(
            lines,
            vec![
                "tunnel add kz-a1b2c mode gre local 203.0.113.5 remote 198.51.100.7 key 4242 ttl 64",
                "addr add 10.77.0.1/30 dev kz-a1b2c",
                "link set kz-a1b2c mtu 1476 up",
            ]
        );
    }

    #[test]
    fn every_field_is_checked() {
        let bad = |f: &dyn Fn(&mut NetSpec)| {
            let mut s = spec();
            f(&mut s);
            validate(&s).is_err()
        };
        assert!(bad(&|s| s.name = "eth0".into()), "only kz- names");
        assert!(bad(&|s| s.name = "kz-".into()));
        assert!(bad(&|s| s.name = "kz-toolongname".into()));
        assert!(bad(&|s| s.name = "kz-a b".into()));
        assert!(bad(&|s| s.name = "kz-A1".into()));
        assert!(bad(&|s| s.name = "kz-a;reboot".into()));
        assert!(bad(&|s| s.local = "127.0.0.1".into()));
        assert!(bad(&|s| s.remote = "0.0.0.0".into()));
        assert!(bad(&|s| s.remote = "224.0.0.1".into()));
        assert!(bad(&|s| s.remote = "$(reboot)".into()));
        assert!(bad(&|s| s.remote = s.local.clone()));
        assert!(bad(&|s| s.local = "::1".into()));
        assert!(bad(&|s| s.key = 0));
        assert!(bad(&|s| s.mtu = 1500), "GRE needs room for its header");
        assert!(bad(&|s| s.mtu = 100));
        assert!(bad(&|s| s.prefix = 24));
        assert!(
            bad(&|s| s.peer = "10.77.0.9".into()),
            "peer outside the /30"
        );
        assert!(bad(&|s| s.peer = s.address.clone()));
        assert!(bad(&|s| s.address = "127.0.0.1".into()));
        assert!(validate(&spec()).is_ok());
        let mut p2p = spec();
        (p2p.prefix, p2p.address, p2p.peer) = (31, "10.77.0.0".into(), "10.77.0.1".into());
        assert!(validate(&p2p).is_ok());
    }

    #[test]
    fn a_join_codes_link_is_kept_once_and_replaced_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let net = Net::with_exec(Some(dir.path().join("net.toml")), fake.clone());
        assert!(net.keep(spec()).unwrap(), "a new link");
        assert!(!net.keep(spec()).unwrap(), "the same one again");
        let moved = NetSpec {
            remote: "198.51.100.8".into(),
            ..spec()
        };
        assert!(net.keep(moved.clone()).unwrap(), "new settings");
        assert_eq!(net.load().link, vec![moved]);
        // Nothing ran: the agent makes it when it starts.
        assert!(calls(&fake).is_empty());
        assert!(net
            .keep(NetSpec {
                name: "eth0".into(),
                ..spec()
            })
            .is_err());
    }

    #[test]
    fn interface_names() {
        for good in ["kz-a", "kz-a1b2c", "kz-12345678"] {
            assert!(valid_ifname(good), "{good}");
        }
        for bad in [
            "kz",
            "kz-",
            "eth0",
            "kz-123456789",
            "kz-a-b",
            "wg0",
            "kz_a",
            "",
        ] {
            assert!(!valid_ifname(bad), "{bad}");
        }
    }

    #[test]
    fn ping_output_is_read() {
        let out = "3 packets transmitted, 3 received, 0% packet loss, time 2003ms\nrtt min/avg/max/mdev = 0.412/0.598/0.821/0.170 ms\n";
        assert_eq!(parse_ping(out), Some(0.598));
        assert_eq!(parse_ping("100% packet loss"), None);
    }

    #[tokio::test]
    async fn up_replaces_an_old_link_runs_the_commands_and_keeps_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("net.toml");
        let fake = Arc::new(Fake::default());
        let net = Net::with_exec(Some(file.clone()), fake.clone());
        net.up(spec()).await.unwrap();
        let c = calls(&fake);
        assert_eq!(c[0], "ip link del kz-a1b2c", "an old one is removed first");
        assert!(c[1].starts_with("ip tunnel add kz-a1b2c mode gre"));
        assert_eq!(c.len(), 4);
        // the same link twice is one entry in the list
        net.up(spec()).await.unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        assert_eq!(saved.matches("[[link]]").count(), 1);
        assert!(saved.contains("kz-a1b2c") && saved.contains("4242"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn a_step_that_fails_removes_the_half_made_interface_and_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("net.toml");
        let fake = Arc::new(Fake {
            fail_on: Some("addr add"),
            ..Fake::default()
        });
        let net = Net::with_exec(Some(file.clone()), fake.clone());
        let err = net.up(spec()).await.unwrap_err();
        assert!(format!("{err:#}").contains("addr add"));
        assert_eq!(calls(&fake).last().unwrap(), "ip link del kz-a1b2c");
        assert!(!file.exists());
        // a bad request runs nothing at all
        let mut bad = spec();
        bad.name = "eth0".into();
        let before = calls(&fake).len();
        assert!(net.up(bad).await.is_err());
        assert_eq!(calls(&fake).len(), before);
    }

    #[tokio::test]
    async fn down_forgets_the_link_and_only_touches_kz_interfaces() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("net.toml");
        let fake = Arc::new(Fake::default());
        let net = Net::with_exec(Some(file.clone()), fake.clone());
        net.up(spec()).await.unwrap();
        net.down("kz-a1b2c").await.unwrap();
        assert_eq!(calls(&fake).last().unwrap(), "ip link del kz-a1b2c");
        assert_eq!(
            std::fs::read_to_string(&file)
                .unwrap()
                .matches("[[link]]")
                .count(),
            0
        );
        let before = calls(&fake).len();
        assert!(net.down("eth0").await.is_err());
        assert!(net.down("lo").await.is_err());
        assert_eq!(
            calls(&fake).len(),
            before,
            "nothing ran for a name that is not ours"
        );
    }

    #[tokio::test]
    async fn saved_links_come_back_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("net.toml");
        Net::with_exec(Some(file.clone()), Arc::new(Fake::default()))
            .up(spec())
            .await
            .unwrap();
        // a fresh agent process, after a reboot
        let fake = Arc::new(Fake::default());
        let after = Net::with_exec(Some(file), fake.clone());
        let done = after.restore().await;
        assert_eq!(done.len(), 1);
        assert!(done[0].1.is_ok());
        assert!(calls(&fake)
            .iter()
            .any(|c| c.starts_with("ip tunnel add kz-a1b2c")));
    }

    #[tokio::test]
    async fn a_restart_leaves_an_interface_that_is_already_there_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("net.toml");
        Net::with_exec(Some(file.clone()), Arc::new(Fake::default()))
            .up(spec())
            .await
            .unwrap();
        // The agent restarts and the kernel still has the interface: it is only set up, so
        // the tunnels over it do not lose their connections for an update.
        let sys = dir.path().join("sys");
        std::fs::create_dir_all(sys.join("kz-a1b2c")).unwrap();
        let fake = Arc::new(Fake::default());
        let after = Net::with_exec(Some(file.clone()), fake.clone()).with_sys(sys.clone());
        let done = after.restore().await;
        assert!(done.len() == 1 && done[0].1.is_ok());
        assert_eq!(calls(&fake), vec!["ip link set kz-a1b2c up".to_owned()]);
        // After a reboot it is not there, and it is made again.
        let fake = Arc::new(Fake::default());
        let reboot = Net::with_exec(Some(file), fake.clone()).with_sys(dir.path().join("empty"));
        assert!(reboot.restore().await[0].1.is_ok());
        assert!(calls(&fake)
            .iter()
            .any(|c| c.starts_with("ip tunnel add kz-a1b2c mode gre")));
    }

    #[tokio::test]
    async fn the_path_test_pings_the_peer_through_the_interface() {
        let fake = Arc::new(Fake {
            ping: "rtt min/avg/max/mdev = 1.0/2.5/4.0/0.5 ms\n",
            ..Fake::default()
        });
        let net = Net::with_exec(None, fake.clone());
        assert!(!net.ping("kz-a1b2c").await.ok, "unknown until it is up");
        net.up(spec()).await.unwrap();
        // without a state file the list is not kept, so the ping cannot find it
        assert!(!net.ping("kz-a1b2c").await.ok);
        let dir = tempfile::tempdir().unwrap();
        let net = Net::with_exec(Some(dir.path().join("net.toml")), fake.clone());
        net.up(spec()).await.unwrap();
        let r = net.ping("kz-a1b2c").await;
        assert!(r.ok);
        assert_eq!(r.rtt_ms, Some(2.5));
        assert_eq!(
            calls(&fake).last().unwrap(),
            "ping -c 3 -W 2 -I kz-a1b2c 10.77.0.2"
        );
        let blocked = Net::with_exec(
            Some(dir.path().join("net.toml")),
            Arc::new(Fake {
                fail_on: Some("ping"),
                ..Fake::default()
            }),
        );
        assert!(!blocked.ping("kz-a1b2c").await.ok);
    }

    #[tokio::test]
    async fn status_says_whether_gre_can_be_made() {
        let ok = Net::with_exec(None, Arc::new(Fake::default()));
        assert!(ok.status().await.gre);
        let fake = Arc::new(Fake {
            fail_on: Some("tunnel add"),
            ..Fake::default()
        });
        let no = Net::with_exec(None, fake).status().await;
        assert!(!no.gre && no.reason.is_some());
    }
}
