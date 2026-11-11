//! The egress side of `myip local`: which local address the operating system would use to
//! reach the Internet, and which default gateway it would send that traffic to.
//!
//! The address comes from asking the kernel to route a UDP socket; a connected UDP socket
//! sends nothing, so no packet leaves the machine. The gateway comes from the Linux
//! routing tables (`/proc/net/route`, `/proc/net/ipv6_route`) and is simply unknown on
//! other platforms.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

/// Where the egress question is "asked": Cloudflare's resolver, already one of the public
/// providers. Only the routing decision matters; nothing is sent to it.
pub const PROBE_V4: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
pub const PROBE_V6: SocketAddr = SocketAddr::new(
    IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111)),
    53,
);

/// Asks the operating system which local address it would use to reach `target`.
pub trait EgressProbe {
    fn local_address_towards(&self, target: SocketAddr) -> io::Result<IpAddr>;
}

pub struct UdpProbe;

impl EgressProbe for UdpProbe {
    fn local_address_towards(&self, target: SocketAddr) -> io::Result<IpAddr> {
        let wildcard: SocketAddr = match target {
            SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(wildcard)?;
        socket.connect(target)?;
        Ok(socket.local_addr()?.ip())
    }
}

/// A default route: where traffic with no more specific route is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultRoute {
    pub gateway: IpAddr,
    pub interface: String,
    pub metric: u32,
    /// A source-specific route (IPv6 `from <prefix>`) only applies to traffic whose source
    /// address is inside the prefix. `None` is an unconditional default.
    pub source: Option<(Ipv6Addr, u8)>,
}

impl DefaultRoute {
    /// Whether traffic from `address` may take this route.
    fn applies_to(&self, address: &IpAddr) -> bool {
        let Some((prefix, length)) = self.source else {
            return true;
        };
        let IpAddr::V6(address) = address else {
            return false;
        };
        let mask = u128::MAX.checked_shl(128 - u32::from(length)).unwrap_or(0);
        (prefix.to_bits() ^ address.to_bits()) & mask == 0
    }

    fn specificity(&self) -> u8 {
        self.source.map_or(0, |(_, length)| length)
    }
}

const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;

/// `/proc/net/route`: hexadecimal fields printed from native-endian 32-bit words, so the
/// bytes of an address are in memory order.
pub fn parse_ipv4_routes(content: &str) -> Vec<DefaultRoute> {
    let mut routes: Vec<DefaultRoute> = content.lines().filter_map(ipv4_default_route).collect();
    routes.sort_by_key(|route| route.metric);
    routes
}

fn ipv4_default_route(line: &str) -> Option<DefaultRoute> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 8 {
        return None;
    }
    let word = |text: &str| u32::from_str_radix(text, 16).ok();
    let (destination, gateway, flags, mask) = (
        word(fields[1])?,
        word(fields[2])?,
        word(fields[3])?,
        word(fields[7])?,
    );
    let metric: u32 = fields[6].parse().ok()?;
    let is_default = destination == 0 && mask == 0 && gateway != 0;
    if !is_default || flags & (RTF_UP | RTF_GATEWAY) != (RTF_UP | RTF_GATEWAY) {
        return None;
    }
    Some(DefaultRoute {
        gateway: IpAddr::V4(Ipv4Addr::from(gateway.to_ne_bytes())),
        interface: fields[0].to_owned(),
        metric,
        source: None,
    })
}

/// `/proc/net/ipv6_route`: destination, prefix length, source, source prefix length,
/// next hop, metric, refcount, use count, flags (all hexadecimal) and the interface.
pub fn parse_ipv6_routes(content: &str) -> Vec<DefaultRoute> {
    let mut routes: Vec<DefaultRoute> = content.lines().filter_map(ipv6_default_route).collect();
    routes.sort_by_key(|route| route.metric);
    routes
}

fn ipv6_default_route(line: &str) -> Option<DefaultRoute> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 {
        return None;
    }
    let address = |text: &str| {
        if text.len() == 32 {
            u128::from_str_radix(text, 16).ok().map(Ipv6Addr::from)
        } else {
            None
        }
    };
    let destination = address(fields[0])?;
    let prefix_len = u32::from_str_radix(fields[1], 16).ok()?;
    let source = address(fields[2])?;
    let source_len = u8::from_str_radix(fields[3], 16).ok()?;
    let next_hop = address(fields[4])?;
    let metric = u32::from_str_radix(fields[5], 16).ok()?;
    let flags = u32::from_str_radix(fields[8], 16).ok()?;
    let is_default = destination.is_unspecified() && prefix_len == 0 && !next_hop.is_unspecified();
    if !is_default || flags & (RTF_UP | RTF_GATEWAY) != (RTF_UP | RTF_GATEWAY) || source_len > 128 {
        return None;
    }
    Some(DefaultRoute {
        gateway: IpAddr::V6(next_hop),
        interface: fields[9].to_owned(),
        metric,
        source: (source_len > 0).then_some((source, source_len)),
    })
}

/// The gateway traffic from `address` would use on `interface`: among the default routes
/// on that interface whose source constraint (if any) contains `address`, the most
/// specific source wins, then the lowest metric. When the best candidates disagree on the
/// gateway, or none applies, or the interface is unknown, the answer is `None` rather than
/// a guess.
pub fn gateway_for<'a>(
    routes: &'a [DefaultRoute],
    interface: Option<&str>,
    address: &IpAddr,
) -> Option<&'a DefaultRoute> {
    let interface = interface?;
    let applicable = || {
        routes
            .iter()
            .filter(|route| route.interface == interface && route.applies_to(address))
    };
    let best = applicable()
        .map(|route| (std::cmp::Reverse(route.specificity()), route.metric))
        .min()?;
    let mut tied =
        applicable().filter(|route| (std::cmp::Reverse(route.specificity()), route.metric) == best);
    let first = tied.next()?;
    tied.all(|route| route.gateway == first.gateway)
        .then_some(first)
}

#[cfg(test)]
pub(crate) mod testing {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::*;

    /// Targets with no configured answer fail like a machine with no route.
    #[derive(Default)]
    pub struct FakeProbe {
        found: BTreeMap<SocketAddr, IpAddr>,
        asked: RefCell<Vec<SocketAddr>>,
    }

    impl FakeProbe {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn route(mut self, target: SocketAddr, local: &str) -> Self {
            self.found.insert(target, local.parse().unwrap());
            self
        }

        pub fn asked(&self) -> Vec<SocketAddr> {
            self.asked.borrow().clone()
        }
    }

    impl EgressProbe for FakeProbe {
        fn local_address_towards(&self, target: SocketAddr) -> io::Result<IpAddr> {
            self.asked.borrow_mut().push(target);
            match self.found.get(&target) {
                Some(ip) => Ok(*ip),
                None => Err(io::Error::from(io::ErrorKind::NetworkUnreachable)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;

    use super::*;

    const ROUTE_HEADER: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

    fn v4(lines: &[&str]) -> String {
        format!("{ROUTE_HEADER}{}\n", lines.join("\n"))
    }

    #[test]
    fn a_docker_default_route_is_the_bridge_gateway() {
        let table = v4(&[
            "eth0\t00000000\t010011AC\t0003\t0\t0\t0\t00000000\t0\t0\t0",
            "eth0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0",
        ]);
        assert_eq!(
            parse_ipv4_routes(&table),
            vec![DefaultRoute {
                gateway: "172.17.0.1".parse().unwrap(),
                interface: "eth0".to_owned(),
                metric: 0,
                source: None,
            }]
        );
    }

    #[test]
    fn little_endian_hex_becomes_dotted_quad() {
        let table = v4(&["wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0"]);
        assert_eq!(
            parse_ipv4_routes(&table)[0].gateway,
            "192.168.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn only_up_default_routes_that_are_not_rejects_count() {
        let table = v4(&[
            // not UP
            "eth1\t00000000\t0100000A\t0002\t0\t0\t10\t00000000\t0\t0\t0",
            // a direct default (`default dev eth2`): UP, no gateway
            "eth2\t00000000\t00000000\t0001\t0\t0\t10\t00000000\t0\t0\t0",
            // a specific network, not the default
            "eth3\t0000A8C0\t0100A8C0\t0003\t0\t0\t10\t00FFFFFF\t0\t0\t0",
            // a default destination with a non-default mask
            "eth4\t00000000\t0100A8C0\t0003\t0\t0\t10\t000000FF\t0\t0\t0",
            // `unreachable default`: UP and REJECT
            "*\t00000000\t00000000\t0201\t0\t0\t10\t00000000\t0\t0\t0",
            // the gateway flag without a gateway, and a gateway without the flag
            "eth5\t00000000\t00000000\t0003\t0\t0\t10\t00000000\t0\t0\t0",
            "eth6\t00000000\t0100000A\t0001\t0\t0\t10\t00000000\t0\t0\t0",
            // the real one
            "eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0",
        ]);
        let routes = parse_ipv4_routes(&table);
        let found: Vec<(&str, u32)> = routes
            .iter()
            .map(|route| (route.interface.as_str(), route.metric))
            .collect();
        assert_eq!(found, [("eth2", 10), ("eth0", 100)], "{routes:?}");
    }

    #[test]
    fn malformed_ipv4_rows_and_the_header_are_skipped() {
        let table = v4(&[
            "short\t00000000",
            "eth0\tzzzzzzzz\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0",
            "eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0",
        ]);
        assert_eq!(parse_ipv4_routes(&table).len(), 1);
        assert!(parse_ipv4_routes("").is_empty());
        assert!(parse_ipv4_routes("garbage\n\u{0}\u{1}").is_empty());
    }

    #[test]
    fn several_default_routes_are_all_returned_in_metric_order() {
        let table = v4(&[
            "wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0",
            "eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0",
        ]);
        let routes = parse_ipv4_routes(&table);
        let interfaces: Vec<&str> = routes.iter().map(|r| r.interface.as_str()).collect();
        assert_eq!(interfaces, ["eth0", "wlan0"]);
    }

    const V6_DEFAULT: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000a0027fffe7d0e5b 00000064 00000001 00000000 00450003 eth0";

    #[test]
    fn an_ipv6_router_advertisement_default_route_yields_the_link_local_gateway() {
        let table = format!(
            "{V6_DEFAULT}\n\
             fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001 eth0\n\
             00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000002 00000000 80200001 lo\n"
        );
        let routes = parse_ipv6_routes(&table);
        assert_eq!(
            routes,
            vec![DefaultRoute {
                gateway: "fe80::a00:27ff:fe7d:e5b".parse().unwrap(),
                interface: "eth0".to_owned(),
                metric: 100,
                source: None,
            }]
        );
    }

    #[test]
    fn ipv6_reject_routes_and_non_defaults_are_ignored() {
        let table = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200 lo\n\
                     20010db8000000000000000000000000 20 00000000000000000000000000000000 00 fe80000000000000a00027fffe7d0e5b 00000064 00000001 00000000 00000003 eth0\n\
                     not a route\n";
        assert!(parse_ipv6_routes(table).is_empty());
        assert!(parse_ipv6_routes("").is_empty());
    }

    #[test]
    fn a_gateway_flag_without_a_gateway_address_is_malformed() {
        let table = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000064 00000001 00000000 00000003 eth0\n";
        assert!(parse_ipv6_routes(table).is_empty());
        let address_without_flag = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000064 00000001 00000000 00000001 eth0\n";
        assert!(parse_ipv6_routes(address_without_flag).is_empty());
    }

    #[test]
    fn an_ipv6_direct_default_is_parsed_but_a_reject_default_is_not() {
        let table = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 0000000a 00000001 00000000 00000001 wg0\n\
                     00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000064 00000001 00000000 00000201 eth1\n";
        let routes = parse_ipv6_routes(table);
        let found: Vec<(&str, u32)> = routes
            .iter()
            .map(|route| (route.interface.as_str(), route.metric))
            .collect();
        assert_eq!(found, [("wg0", 10)], "{routes:?}");
    }

    fn route(interface: &str, gateway: &str, metric: u32) -> DefaultRoute {
        DefaultRoute {
            gateway: gateway.parse().unwrap(),
            interface: interface.to_owned(),
            metric,
            source: None,
        }
    }

    fn from_source(mut route: DefaultRoute, prefix: &str, length: u8) -> DefaultRoute {
        route.source = Some((prefix.parse().unwrap(), length));
        route
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn gateway_of(routes: &[DefaultRoute], interface: &str, address: &str) -> Option<IpAddr> {
        gateway_for(routes, Some(interface), &ip(address)).map(|r| r.gateway)
    }

    #[test]
    fn the_gateway_is_the_default_route_on_the_egress_interface() {
        let routes = [
            route("utun3", "10.8.0.1", 10),
            route("en0", "192.168.0.1", 600),
        ];
        assert_eq!(
            gateway_of(&routes, "en0", "192.168.0.20"),
            Some(ip("192.168.0.1"))
        );
    }

    #[test]
    fn the_lowest_metric_wins_on_one_interface() {
        let routes = [
            route("eth0", "10.0.0.254", 200),
            route("eth0", "10.0.0.1", 100),
        ];
        assert_eq!(
            gateway_of(&routes, "eth0", "10.0.0.5"),
            Some(ip("10.0.0.1"))
        );
    }

    #[test]
    fn no_known_interface_or_no_route_on_it_means_no_gateway() {
        let routes = [route("eth0", "10.0.0.1", 100)];
        let address = ip("10.0.0.5");
        assert!(gateway_for(&routes, None, &address).is_none());
        assert!(gateway_for(&routes, Some("wlan0"), &address).is_none());
        assert!(gateway_for(&[], Some("eth0"), &address).is_none());
    }

    #[test]
    fn a_source_specific_route_only_applies_inside_its_prefix() {
        let routes = [from_source(
            route("eth0", "fe80::1", 100),
            "2001:db8:1::",
            64,
        )];
        assert_eq!(
            gateway_of(&routes, "eth0", "2001:db8:1::7"),
            Some(ip("fe80::1"))
        );
        assert_eq!(gateway_of(&routes, "eth0", "2001:db8:2::7"), None);
        assert_eq!(gateway_of(&routes, "eth0", "fd00::7"), None);
        assert_eq!(gateway_of(&routes, "eth0", "192.168.0.7"), None);
    }

    #[test]
    fn the_source_prefix_length_is_honoured_bit_by_bit() {
        let at = |length| {
            [from_source(
                route("eth0", "fe80::1", 1),
                "2001:db8:8000::",
                length,
            )]
        };
        assert!(gateway_of(&at(1), "eth0", "7fff::1").is_some());
        assert!(gateway_of(&at(1), "eth0", "8000::1").is_none());
        assert!(gateway_of(&at(64), "eth0", "2001:db8:8000:1::1").is_none());
        assert!(gateway_of(&at(63), "eth0", "2001:db8:8000:1::1").is_some());
        assert!(gateway_of(&at(128), "eth0", "2001:db8:8000::").is_some());
        assert!(gateway_of(&at(128), "eth0", "2001:db8:8000::1").is_none());
    }

    #[test]
    fn the_most_specific_source_beats_a_better_metric() {
        let routes = [
            route("eth0", "fe80::a", 50),
            from_source(route("eth0", "fe80::b", 900), "2001:db8:2::", 64),
            from_source(route("eth0", "fe80::c", 100), "2001:db8::", 32),
        ];
        assert_eq!(
            gateway_of(&routes, "eth0", "2001:db8:2::5"),
            Some(ip("fe80::b"))
        );
        assert_eq!(
            gateway_of(&routes, "eth0", "2001:db8:3::5"),
            Some(ip("fe80::c"))
        );
        assert_eq!(gateway_of(&routes, "eth0", "fd00::5"), Some(ip("fe80::a")));
    }

    #[test]
    fn equally_good_routes_to_different_gateways_are_ambiguous() {
        let two = [route("eth0", "fe80::a", 100), route("eth0", "fe80::b", 100)];
        assert_eq!(gateway_of(&two, "eth0", "2001:db8::5"), None);
        let same = [route("eth0", "fe80::a", 100), route("eth0", "fe80::a", 100)];
        assert_eq!(
            gateway_of(&same, "eth0", "2001:db8::5"),
            Some(ip("fe80::a"))
        );
        let tie_then_better = [
            route("eth0", "fe80::a", 100),
            route("eth0", "fe80::b", 100),
            route("eth0", "fe80::c", 10),
        ];
        assert_eq!(
            gateway_of(&tie_then_better, "eth0", "2001:db8::5"),
            Some(ip("fe80::c"))
        );
    }

    #[test]
    fn ipv6_source_fields_are_parsed_into_the_route() {
        let line = |src: &str, len: &str| {
            format!(
                "00000000000000000000000000000000 00 {src} {len} fe800000000000000000000000000001 00000064 00000001 00000000 00450003 eth0\n"
            )
        };
        let source = "20010db8000100000000000000000000";
        let routes = parse_ipv6_routes(&line(source, "40"));
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].source,
            Some(("2001:db8:1::".parse().unwrap(), 64))
        );
        let routes = parse_ipv6_routes(&line("00000000000000000000000000000000", "00"));
        assert_eq!(routes[0].source, None);
        // A prefix longer than 128 bits is not a route; neither is a garbled source.
        assert!(parse_ipv6_routes(&line(source, "81")).is_empty());
        assert!(parse_ipv6_routes(&line("zz", "00")).is_empty());
    }

    fn hex4(address: &str) -> String {
        let octets = address.parse::<Ipv4Addr>().unwrap().octets();
        format!("{:08X}", u32::from_ne_bytes(octets))
    }

    /// A `/proc/net/route` row for `default via <gateway> dev <interface> metric <metric>`.
    fn v4_via(interface: &str, gateway: &str, metric: u32) -> String {
        let gateway = hex4(gateway);
        format!("{interface}\t00000000\t{gateway}\t0003\t0\t0\t{metric}\t00000000\t0\t0\t0")
    }

    /// A `/proc/net/route` row for `default dev <interface> metric <metric>`.
    fn v4_direct(interface: &str, metric: u32) -> String {
        format!("{interface}\t00000000\t00000000\t0001\t0\t0\t{metric}\t00000000\t0\t0\t0")
    }

    fn v4_gateway(rows: &[String], interface: &str) -> Option<IpAddr> {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        gateway_of(&parse_ipv4_routes(&v4(&rows)), interface, "10.0.0.5")
    }

    #[test]
    fn a_direct_default_is_parsed_alongside_a_gateway_default() {
        let table = v4(&[&v4_via("eth0", "10.0.0.1", 100), &v4_direct("wg0", 10)]);
        let interfaces: Vec<String> = parse_ipv4_routes(&table)
            .into_iter()
            .map(|route| route.interface)
            .collect();
        assert_eq!(interfaces, ["wg0", "eth0"]);
    }

    #[test]
    fn a_direct_default_with_the_lower_metric_wins_so_there_is_no_gateway() {
        let rows = [v4_direct("eth0", 10), v4_via("eth0", "10.0.0.1", 100)];
        assert_eq!(v4_gateway(&rows, "eth0"), None);
    }

    #[test]
    fn a_gateway_default_with_a_strictly_lower_metric_beats_a_direct_one() {
        let rows = [v4_direct("eth0", 100), v4_via("eth0", "10.0.0.1", 10)];
        assert_eq!(v4_gateway(&rows, "eth0"), Some(ip("10.0.0.1")));
    }

    #[test]
    fn a_direct_default_tied_with_a_gateway_default_gives_no_gateway() {
        let rows = [v4_via("eth0", "10.0.0.1", 100), v4_direct("eth0", 100)];
        assert_eq!(v4_gateway(&rows, "eth0"), None);
    }

    #[test]
    fn direct_and_gateway_defaults_on_other_interfaces_are_left_alone() {
        let rows = [
            v4_direct("wg0", 10),
            v4_via("eth0", "10.0.0.1", 100),
            v4_direct("eth0", 200),
            v4_via("eth1", "10.1.0.1", 50),
        ];
        assert_eq!(v4_gateway(&rows, "wg0"), None);
        assert_eq!(v4_gateway(&rows, "eth0"), Some(ip("10.0.0.1")));
        assert_eq!(v4_gateway(&rows, "eth1"), Some(ip("10.1.0.1")));
        assert_eq!(v4_gateway(&rows, "eth2"), None);
    }

    /// A `/proc/net/ipv6_route` row for a default route; no gateway is a direct default.
    fn v6_row(
        source: Option<(&str, u8)>,
        gateway: Option<&str>,
        metric: u32,
        interface: &str,
    ) -> String {
        let bits = |text: &str| text.parse::<Ipv6Addr>().unwrap().to_bits();
        let (source, length) = source.map_or((0, 0), |(prefix, length)| (bits(prefix), length));
        let (next_hop, flags) = gateway.map_or((0, 0x1), |gateway| (bits(gateway), 0x3));
        format!(
            "{:032x} 00 {source:032x} {length:02x} {next_hop:032x} {metric:08x} 00000001 00000000 {flags:08x} {interface}\n",
            0u128
        )
    }

    fn v6_gateway(rows: &[String], interface: &str, address: &str) -> Option<IpAddr> {
        gateway_of(&parse_ipv6_routes(&rows.concat()), interface, address)
    }

    #[test]
    fn an_ipv6_direct_default_with_the_lower_metric_wins_so_there_is_no_gateway() {
        let rows = [
            v6_row(None, None, 10, "eth0"),
            v6_row(None, Some("fe80::1"), 100, "eth0"),
        ];
        assert_eq!(v6_gateway(&rows, "eth0", "2001:db8::5"), None);
        let tie = [
            v6_row(None, Some("fe80::1"), 100, "eth0"),
            v6_row(None, None, 100, "eth0"),
        ];
        assert_eq!(v6_gateway(&tie, "eth0", "2001:db8::5"), None);
        let backup_better = [
            v6_row(None, None, 100, "eth0"),
            v6_row(None, Some("fe80::1"), 10, "eth0"),
        ];
        assert_eq!(
            v6_gateway(&backup_better, "eth0", "2001:db8::5"),
            Some(ip("fe80::1"))
        );
    }

    #[test]
    fn a_source_specific_direct_default_only_applies_to_its_own_source() {
        let rows = [
            v6_row(None, Some("fe80::a"), 100, "eth0"),
            v6_row(Some(("2001:db8:2::", 64)), None, 10, "eth0"),
        ];
        // Inside its prefix the kernel prefers the source-specific route, which is direct:
        // there is no gateway to report, and the unconditional one is not it.
        assert_eq!(v6_gateway(&rows, "eth0", "2001:db8:2::5"), None);
        // Outside it the source-specific direct default does not apply at all.
        assert_eq!(
            v6_gateway(&rows, "eth0", "2001:db8:3::5"),
            Some(ip("fe80::a"))
        );
    }

    #[test]
    fn an_unconditional_direct_default_loses_to_a_matching_source_specific_gateway() {
        let rows = [
            v6_row(None, None, 10, "eth0"),
            v6_row(Some(("2001:db8:2::", 64)), Some("fe80::2"), 100, "eth0"),
        ];
        assert_eq!(
            v6_gateway(&rows, "eth0", "2001:db8:2::5"),
            Some(ip("fe80::2"))
        );
        assert_eq!(v6_gateway(&rows, "eth0", "2001:db8:3::5"), None);
    }

    #[test]
    fn ipv6_direct_and_gateway_defaults_on_different_interfaces_do_not_interfere() {
        let rows = [
            v6_row(None, None, 10, "wg0"),
            v6_row(None, Some("fe80::1"), 100, "eth0"),
        ];
        assert_eq!(v6_gateway(&rows, "wg0", "2001:db8::5"), None);
        assert_eq!(
            v6_gateway(&rows, "eth0", "2001:db8::5"),
            Some(ip("fe80::1"))
        );
    }

    #[test]
    fn the_udp_probe_reports_the_address_the_kernel_would_use() {
        let target: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert_eq!(
            UdpProbe.local_address_towards(target).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn the_udp_probe_sends_nothing() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let target = listener.local_addr().unwrap();
        UdpProbe.local_address_towards(target).unwrap();
        let mut buffer = [0u8; 16];
        let err = listener.recv_from(&mut buffer).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn the_probe_targets_are_cloudflares_resolver_over_both_families() {
        assert_eq!(PROBE_V4.to_string(), "1.1.1.1:53");
        assert_eq!(PROBE_V6.to_string(), "[2606:4700:4700::1111]:53");
    }

    #[test]
    fn the_fake_probe_records_questions_and_fails_without_a_route() {
        let probe = testing::FakeProbe::new().route(PROBE_V4, "192.168.1.20");
        assert_eq!(
            probe.local_address_towards(PROBE_V4).unwrap(),
            "192.168.1.20".parse::<IpAddr>().unwrap()
        );
        assert!(probe.local_address_towards(PROBE_V6).is_err());
        assert_eq!(probe.asked(), vec![PROBE_V4, PROBE_V6]);
    }
}
