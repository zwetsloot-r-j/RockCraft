//! Home-network helpers: which peers the share server talks to, this
//! machine's LAN addresses, and its display name.

use std::net::{IpAddr, Ipv4Addr};

/// Whether `ip` is on the home network: loopback, RFC 1918 private, IPv4
/// link-local, IPv6 unique-local (`fc00::/7`) or link-local (`fe80::/10`).
/// IPv4-mapped IPv6 addresses are judged as their IPv4 address. The share
/// server answers only these peers (there is no other auth, by decision).
pub fn is_home_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_home_network(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// This machine's private IPv4 addresses (no loopback, no link-local),
/// sorted — what another device on the network types to connect.
pub fn lan_addresses() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(v4) if v4.is_private() => Some(v4),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The name this machine shares under: `COMPUTERNAME` (Windows) or
/// `HOSTNAME`, else `/etc/hostname`, else `"RockCraft"`.
pub fn host_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .chain(std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "RockCraft".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_network_filter() {
        for ok in [
            "192.168.1.2",
            "10.0.0.1",
            "172.16.5.4",
            "169.254.1.1",
            "127.0.0.1",
            "fd00::1",
            "fe80::1",
            "::1",
            "::ffff:192.168.0.9",
        ] {
            assert!(is_home_network(ok.parse().unwrap()), "{ok} should pass");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(
                !is_home_network(bad.parse().unwrap()),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn lan_addresses_are_private_ipv4() {
        for ip in lan_addresses() {
            assert!(ip.is_private());
        }
    }
}
