//! Entry side: how many user connections and UDP flows one client address may hold at
//! once (`tuning.max_conns_per_ip`), over all forward rules of a tunnel.
//!
//! Off by default: on mobile networks many users share one address (carrier NAT), so a
//! limit is something the admin chooses for their own users.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tracing::warn;

/// At most one warning about refused clients this often.
const WARN_INTERVAL: Duration = Duration::from_secs(30);

pub struct PerIp {
    max: u32,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    open: HashMap<IpAddr, u32>,
    last_warning: Option<Instant>,
}

impl PerIp {
    pub fn new(max: u32) -> Arc<Self> {
        Arc::new(Self {
            max,
            state: Mutex::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes a slot for `ip`, held until the slot is dropped; `None` (and a warning now
    /// and then) when the address already has `max` open.
    pub fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<Slot> {
        let ip = ip.to_canonical();
        let mut st = self.lock();
        let open = st.open.entry(ip).or_default();
        if *open >= self.max {
            let now = Instant::now();
            if st
                .last_warning
                .map_or(true, |t| now.duration_since(t) >= WARN_INTERVAL)
            {
                st.last_warning = Some(now);
                warn!(
                    client = %ip,
                    max = self.max,
                    "a client reached tuning.max_conns_per_ip; its new connections are closed"
                );
            }
            return None;
        }
        *open += 1;
        Some(Slot {
            limit: self.clone(),
            ip,
        })
    }

    #[cfg(test)]
    fn open(&self, ip: IpAddr) -> u32 {
        self.lock().open.get(&ip).copied().unwrap_or(0)
    }
}

/// A connection or flow counted against its address until dropped.
pub struct Slot {
    limit: Arc<PerIp>,
    ip: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut st = self.limit.lock();
        if let Some(open) = st.open.get_mut(&self.ip) {
            *open -= 1;
            if *open == 0 {
                st.open.remove(&self.ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_gets_up_to_the_limit_and_slots_come_back() {
        let limit = PerIp::new(2);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let first = limit.acquire(a).unwrap();
        let _second = limit.acquire(a).unwrap();
        assert!(limit.acquire(a).is_none());
        // Another address is not affected.
        assert!(limit.acquire(b).is_some());
        drop(first);
        assert!(limit.acquire(a).is_some());
    }

    #[test]
    fn an_ipv4_address_counts_the_same_through_ipv6() {
        let limit = PerIp::new(1);
        let v4: IpAddr = "192.0.2.7".parse().unwrap();
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        let slot = limit.acquire(v4).unwrap();
        assert!(limit.acquire(mapped).is_none());
        drop(slot);
        assert_eq!(limit.open(v4), 0);
        assert!(
            limit.lock().open.is_empty(),
            "no entry left for a gone address"
        );
    }
}
