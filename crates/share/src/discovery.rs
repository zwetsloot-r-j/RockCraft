//! Finding sharing peers on the home network without typing addresses
//! (M26-C): mDNS/DNS-SD, the mechanism printers and Chromecasts use.
//!
//! A sharing instance advertises `_rockcraft._tcp.local.` with TXT `api=1`
//! and `name=<host name>`; [`discover`] browses for a moment and returns the
//! peers it resolved. When multicast isn't available (no interface, port 5353
//! blocked, router isolation) both sides degrade quietly: sharing still works
//! by typed `host:port`.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

use crate::net::lan_addresses;
use crate::server::API_VERSION;

/// The DNS-SD service type RockCraft shares under.
pub const SERVICE_TYPE: &str = "_rockcraft._tcp.local.";
/// Default browse time for [`discover`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1500);

/// A sharing RockCraft found on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Peer {
    /// Its display name (the host name it shares under).
    pub name: String,
    /// `ip:port` (`[ipv6]:port` when it has no IPv4) — what the client uses.
    pub addr: String,
    /// Its HTTP API version.
    pub api: u32,
}

/// Keeps this instance advertised while alive; unregisters on drop.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// Advertise a share server called `name` on `port`. Fails (with a
    /// human-readable reason) when the mDNS daemon can't start.
    pub fn start(name: &str, port: u16) -> Result<Advertiser, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let label: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let host = format!("{}.local.", label.trim_matches('-').to_ascii_lowercase());
        // The port keeps two instances on one machine distinct.
        let instance = format!("{name} {port}");
        let api = API_VERSION.to_string();
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &host,
            "",
            port,
            &[("api", api.as_str()), ("name", name)][..],
        )
        .map_err(|e| e.to_string())?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(Advertiser { daemon, fullname })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// One resolved advertisement, before filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub name: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    pub api: u32,
}

/// Browse for `timeout` and return the sharing peers found, excluding this
/// instance (`own_port` = the port we share on, if we do). Empty when mDNS
/// is unavailable — callers fall back to typed addresses.
pub fn discover(timeout: Duration, own_port: Option<u16>) -> Vec<Peer> {
    let Ok(daemon) = ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(events) = daemon.browse(SERVICE_TYPE) else {
        let _ = daemon.shutdown();
        return Vec::new();
    };
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(r)) => found.push(Resolved {
                name: r
                    .get_property_val_str("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| r.host.trim_end_matches('.').to_string()),
                port: r.port,
                addrs: r.addresses.iter().map(|a| a.to_ip_addr()).collect(),
                api: r
                    .get_property_val_str("api")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            }),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    let mut own: Vec<IpAddr> = lan_addresses().into_iter().map(IpAddr::V4).collect();
    own.push(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    select_peers(found, own_port, &own)
}

/// Turn resolved advertisements into peers: pick one address per peer (a
/// non-loopback IPv4 first, then non-loopback IPv6), drop this instance (our
/// port on one of our addresses), de-duplicate by address, sort by name.
pub(crate) fn select_peers(
    found: Vec<Resolved>,
    own_port: Option<u16>,
    own_addrs: &[IpAddr],
) -> Vec<Peer> {
    let mut peers: Vec<Peer> = Vec::new();
    for r in found {
        let is_self = own_port == Some(r.port) && r.addrs.iter().any(|a| own_addrs.contains(a));
        if is_self {
            continue;
        }
        let pick = r
            .addrs
            .iter()
            .filter(|a| !a.is_loopback())
            .min_by_key(|a| (a.is_ipv6(), **a));
        let Some(ip) = pick else { continue };
        let addr = match ip {
            IpAddr::V4(v4) => format!("{v4}:{}", r.port),
            IpAddr::V6(v6) => format!("[{v6}]:{}", r.port),
        };
        if peers.iter().any(|p| p.addr == addr) {
            continue;
        }
        peers.push(Peer {
            name: r.name,
            addr,
            api: r.api,
        });
    }
    peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.addr.cmp(&b.addr)));
    peers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(name: &str, port: u16, addrs: &[&str]) -> Resolved {
        Resolved {
            name: name.into(),
            port,
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
            api: 1,
        }
    }

    #[test]
    fn prefers_ipv4_and_skips_loopback() {
        let peers = select_peers(
            vec![resolved(
                "piano-pc",
                47800,
                &["fe80::1", "127.0.0.1", "192.168.1.7"],
            )],
            None,
            &[],
        );
        assert_eq!(peers[0].addr, "192.168.1.7:47800");
        let v6 = select_peers(vec![resolved("v6", 47800, &["fd00::7"])], None, &[]);
        assert_eq!(v6[0].addr, "[fd00::7]:47800");
        let none = select_peers(vec![resolved("lo", 47800, &["127.0.0.1"])], None, &[]);
        assert!(none.is_empty());
    }

    #[test]
    fn excludes_this_instance_but_not_others_on_this_machine() {
        let own: Vec<IpAddr> = vec!["192.168.1.5".parse().unwrap()];
        let peers = select_peers(
            vec![
                resolved("me", 47800, &["192.168.1.5"]),
                resolved("me-too", 47801, &["192.168.1.5"]), // a second instance here
                resolved("other", 47800, &["192.168.1.9"]),
            ],
            Some(47800),
            &own,
        );
        let names: Vec<&str> = peers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["me-too", "other"]);
    }

    #[test]
    fn deduplicates_and_sorts() {
        let peers = select_peers(
            vec![
                resolved("zeta", 47800, &["192.168.1.9"]),
                resolved("alpha", 47800, &["192.168.1.8"]),
                resolved("zeta", 47800, &["192.168.1.9"]),
            ],
            None,
            &[],
        );
        let names: Vec<&str> = peers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
    }

    /// Advertise and browse in one process. Needs working multicast, which CI
    /// runners and some VMs lack, so it is opt-in:
    /// `cargo test -p rockcraft-share -- --ignored advertise_then_discover`.
    #[test]
    #[ignore = "needs multicast on the host network"]
    fn advertise_then_discover() {
        let name = format!("rc-test-{}", std::process::id());
        let _ad = Advertiser::start(&name, 45_999).expect("mDNS daemon starts");
        let peers = discover(Duration::from_secs(3), None);
        let me = peers
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("not found among {peers:?}"));
        assert!(me.addr.ends_with(":45999"));
        assert_eq!(me.api, API_VERSION);
    }
}
