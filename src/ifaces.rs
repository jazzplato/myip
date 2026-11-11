//! The interface-enumeration seam behind `myip local`: a small trait, the real
//! implementation over the `if-addrs` crate, and the pure helpers that group, order and
//! label what the operating system reports.

use std::collections::BTreeMap;
use std::io;
use std::net::IpAddr;

/// One address on an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub ip: IpAddr,
    pub prefix_len: u8,
}

/// A network interface with every address it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub index: Option<u32>,
    pub addresses: Vec<Address>,
}

/// One address as the operating system reports it: the platform APIs return a flat list
/// with one row per address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub index: Option<u32>,
    pub ip: IpAddr,
    pub prefix_len: u8,
}

/// Lists the interfaces of the machine this process runs on.
pub trait Interfaces {
    fn list(&self) -> io::Result<Vec<Interface>>;
}

pub struct SystemInterfaces;

impl Interfaces for SystemInterfaces {
    fn list(&self) -> io::Result<Vec<Interface>> {
        let entries = if_addrs::get_if_addrs()?
            .into_iter()
            .map(|found| Entry {
                prefix_len: match &found.addr {
                    if_addrs::IfAddr::V4(v4) => v4.prefixlen,
                    if_addrs::IfAddr::V6(v6) => v6.prefixlen,
                },
                ip: found.addr.ip(),
                index: found.index,
                name: found.name,
            })
            .collect();
        Ok(group(entries))
    }
}

/// Groups a flat list by interface, collapses duplicates and orders the result: by
/// interface index (unknown last), then name; within an interface IPv4 before IPv6, then
/// by address.
pub fn group(entries: Vec<Entry>) -> Vec<Interface> {
    let mut by_name: BTreeMap<String, Interface> = BTreeMap::new();
    for entry in entries {
        let interface = by_name
            .entry(entry.name.clone())
            .or_insert_with(|| Interface {
                name: entry.name,
                index: None,
                addresses: Vec::new(),
            });
        interface.index = interface.index.or(entry.index);
        interface.addresses.push(Address {
            ip: entry.ip,
            prefix_len: entry.prefix_len,
        });
    }
    let mut interfaces: Vec<Interface> = by_name.into_values().collect();
    for interface in &mut interfaces {
        interface
            .addresses
            .sort_by_key(|address| (address.ip, address.prefix_len));
        interface.addresses.dedup();
    }
    interfaces
        .sort_by_key(|interface| (interface.index.unwrap_or(u32::MAX), interface.name.clone()));
    interfaces
}

/// Name prefixes (compared case-insensitively) of interfaces that are usually not a
/// physical NIC: loopback, container bridges and veth pairs, hypervisor switches,
/// VPN/overlay tunnels and Apple's per-purpose pseudo interfaces.
const VIRTUAL_PREFIXES: &[&str] = &[
    "docker",
    "br-",
    "veth",
    "virbr",
    "utun",
    "awdl",
    "llw",
    "tun",
    "tap",
    "tailscale",
    "wg",
    "vmnet",
    "vboxnet",
    "bridge",
    "gif",
    "stf",
    "anpi",
    "cni",
    "flannel",
    "cali",
    "vxlan",
    "kube-",
    "lxcbr",
    "lxdbr",
    "podman",
    "zt",
    "vethernet",
    "loopback pseudo-interface",
];

/// A hint, from the interface name alone, that the interface is a loopback, bridge, tunnel,
/// VPN or container/VM network rather than a physical one. It is a heuristic and can be wrong.
pub fn likely_virtual(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let is_loopback = lower
        .strip_prefix("lo")
        .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()));
    is_loopback
        || VIRTUAL_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A canned interface list, or a canned enumeration failure.
    pub struct FakeInterfaces {
        result: Result<Vec<Interface>, io::ErrorKind>,
    }

    impl FakeInterfaces {
        pub fn with(interfaces: Vec<Interface>) -> Self {
            Self {
                result: Ok(interfaces),
            }
        }

        pub fn failing(kind: io::ErrorKind) -> Self {
            Self { result: Err(kind) }
        }
    }

    impl Interfaces for FakeInterfaces {
        fn list(&self) -> io::Result<Vec<Interface>> {
            match &self.result {
                Ok(interfaces) => Ok(interfaces.clone()),
                Err(kind) => Err(io::Error::from(*kind)),
            }
        }
    }

    pub fn iface(name: &str, index: u32, addresses: &[(&str, u8)]) -> Interface {
        Interface {
            name: name.to_owned(),
            index: Some(index),
            addresses: addresses
                .iter()
                .map(|(ip, prefix_len)| Address {
                    ip: ip.parse().unwrap(),
                    prefix_len: *prefix_len,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, index: Option<u32>, ip: &str, prefix_len: u8) -> Entry {
        Entry {
            name: name.to_owned(),
            index,
            ip: ip.parse().unwrap(),
            prefix_len,
        }
    }

    fn names(interfaces: &[Interface]) -> Vec<&str> {
        interfaces.iter().map(|i| i.name.as_str()).collect()
    }

    fn ips(interface: &Interface) -> Vec<String> {
        interface
            .addresses
            .iter()
            .map(|a| a.ip.to_string())
            .collect()
    }

    #[test]
    fn addresses_are_grouped_under_their_interface() {
        let grouped = group(vec![
            entry("eth0", Some(2), "10.0.0.5", 24),
            entry("lo", Some(1), "127.0.0.1", 8),
            entry("eth0", Some(2), "fe80::1", 64),
        ]);
        assert_eq!(names(&grouped), ["lo", "eth0"]);
        assert_eq!(ips(&grouped[1]), ["10.0.0.5", "fe80::1"]);
        assert_eq!(grouped[1].addresses[0].prefix_len, 24);
        assert_eq!(grouped[1].addresses[1].prefix_len, 64);
        assert_eq!(grouped[1].index, Some(2));
    }

    #[test]
    fn interfaces_are_ordered_by_index_then_name_with_unknown_indexes_last() {
        let grouped = group(vec![
            entry("zeta", None, "10.9.0.1", 24),
            entry("en1", Some(5), "10.1.0.1", 24),
            entry("alpha", None, "10.8.0.1", 24),
            entry("en0", Some(5), "10.2.0.1", 24),
            entry("lo0", Some(1), "127.0.0.1", 8),
        ]);
        assert_eq!(names(&grouped), ["lo0", "en0", "en1", "alpha", "zeta"]);
    }

    #[test]
    fn within_an_interface_ipv4_comes_first_then_addresses_ascend() {
        let grouped = group(vec![
            entry("en0", Some(4), "2001:db8::2", 64),
            entry("en0", Some(4), "192.168.1.20", 24),
            entry("en0", Some(4), "fe80::1", 64),
            entry("en0", Some(4), "10.0.0.2", 24),
            entry("en0", Some(4), "2001:db8::1", 64),
        ]);
        assert_eq!(
            ips(&grouped[0]),
            [
                "10.0.0.2",
                "192.168.1.20",
                "2001:db8::1",
                "2001:db8::2",
                "fe80::1"
            ]
        );
    }

    #[test]
    fn a_repeated_address_is_listed_once() {
        let grouped = group(vec![
            entry("eth0", Some(2), "10.0.0.5", 24),
            entry("eth0", Some(2), "10.0.0.5", 24),
        ]);
        assert_eq!(grouped[0].addresses.len(), 1);
    }

    #[test]
    fn the_index_comes_from_whichever_entry_knows_it() {
        let grouped = group(vec![
            entry("eth0", None, "10.0.0.5", 24),
            entry("eth0", Some(7), "fe80::1", 64),
        ]);
        assert_eq!(grouped[0].index, Some(7));
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(group(Vec::new()).is_empty());
    }

    #[test]
    fn virtual_looking_names_are_flagged() {
        for name in [
            "lo",
            "lo0",
            "LO",
            "docker0",
            "br-1a2b3c4d5e6f",
            "veth1a2b3c",
            "virbr0",
            "utun3",
            "awdl0",
            "llw0",
            "tun0",
            "tap0",
            "tailscale0",
            "wg0",
            "vmnet8",
            "vboxnet0",
            "bridge100",
            "gif0",
            "stf0",
            "anpi0",
            "cni0",
            "flannel.1",
            "cali1234abcd",
            "vxlan0",
            "kube-ipvs0",
            "lxcbr0",
            "lxdbr0",
            "podman0",
            "zt5u4abcdef",
            "vEthernet (WSL)",
            "vEthernet (Default Switch)",
            "Loopback Pseudo-Interface 1",
        ] {
            assert!(likely_virtual(name), "{name} should look virtual");
        }
    }

    #[test]
    fn physical_looking_names_are_not_flagged() {
        for name in [
            "eth0",
            "en0",
            "en1",
            "ens160",
            "enp0s3",
            "eno1",
            "wlan0",
            "wlp2s0",
            "bond0",
            "Ethernet",
            "Wi-Fi",
            "Local Area Connection",
            "local0",
            "loop",
            "",
        ] {
            assert!(!likely_virtual(name), "{name:?} should not look virtual");
        }
    }

    #[test]
    fn the_fake_returns_its_list_or_its_failure() {
        let fake =
            testing::FakeInterfaces::with(vec![testing::iface("eth0", 2, &[("10.0.0.5", 24)])]);
        assert_eq!(fake.list().unwrap()[0].name, "eth0");
        let failing = testing::FakeInterfaces::failing(io::ErrorKind::PermissionDenied);
        assert_eq!(
            failing.list().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
