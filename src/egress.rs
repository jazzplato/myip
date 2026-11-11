//! The egress side of `myip local`: which local address the operating system would use to
//! reach the Internet, and which default gateway it would send that traffic to.
//!
//! The address comes from asking the kernel to route a UDP socket; a connected UDP socket
//! sends nothing, so no packet leaves the machine. The gateway comes from the Linux
//! routing tables (`/proc/net/route`, `/proc/net/ipv6_route`) and is simply unknown on
//! other platforms.

use std::io;
use std::net::{IpAddr, SocketAddr};

/// Where the egress question is "asked": Cloudflare's resolver, already one of the public
/// providers. Only the routing decision matters; nothing is sent to it.
pub const PROBE_V4: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)), 53);
pub const PROBE_V6: SocketAddr = SocketAddr::new(
    IpAddr::V6(std::net::Ipv6Addr::new(
        0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111,
    )),
    53,
);

/// Asks the operating system which local address it would use to reach `target`.
pub trait EgressProbe {
    fn local_address_towards(&self, target: SocketAddr) -> io::Result<IpAddr>;
}

pub struct UdpProbe;

impl EgressProbe for UdpProbe {
    fn local_address_towards(&self, _target: SocketAddr) -> io::Result<IpAddr> {
        unimplemented!()
    }
}

/// A default route: where traffic with no more specific route is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultRoute {
    pub gateway: IpAddr,
    pub interface: String,
    pub metric: u32,
}

pub fn parse_ipv4_routes(_content: &str) -> Vec<DefaultRoute> {
    unimplemented!()
}

pub fn parse_ipv6_routes(_content: &str) -> Vec<DefaultRoute> {
    unimplemented!()
}

/// The default route on `interface`, lowest metric first. With no known egress interface
/// there is nothing to tie a gateway to, so the answer is `None` rather than a guess.
pub fn gateway_for<'a>(
    _routes: &'a [DefaultRoute],
    _interface: Option<&str>,
) -> Option<&'a DefaultRoute> {
    unimplemented!()
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
    fn only_up_gateway_routes_to_everywhere_count() {
        let table = v4(&[
            // not UP
            "eth1\t00000000\t0100000A\t0002\t0\t0\t10\t00000000\t0\t0\t0",
            // not a gateway route
            "eth2\t00000000\t00000000\t0001\t0\t0\t10\t00000000\t0\t0\t0",
            // a specific network, not the default
            "eth3\t0000A8C0\t0100A8C0\t0003\t0\t0\t10\t00FFFFFF\t0\t0\t0",
            // a default destination with a non-default mask
            "eth4\t00000000\t0100A8C0\t0003\t0\t0\t10\t000000FF\t0\t0\t0",
            // the real one
            "eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0",
        ]);
        let routes = parse_ipv4_routes(&table);
        assert_eq!(routes.len(), 1, "{routes:?}");
        assert_eq!(routes[0].interface, "eth0");
        assert_eq!(routes[0].metric, 100);
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
    fn a_default_route_must_carry_a_gateway_address() {
        let table = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000064 00000001 00000000 00000003 eth0\n";
        assert!(parse_ipv6_routes(table).is_empty());
    }

    fn route(interface: &str, gateway: &str, metric: u32) -> DefaultRoute {
        DefaultRoute {
            gateway: gateway.parse().unwrap(),
            interface: interface.to_owned(),
            metric,
        }
    }

    #[test]
    fn the_gateway_is_the_default_route_on_the_egress_interface() {
        let routes = [
            route("utun3", "10.8.0.1", 10),
            route("en0", "192.168.0.1", 600),
        ];
        assert_eq!(
            gateway_for(&routes, Some("en0")).map(|r| r.gateway),
            Some("192.168.0.1".parse().unwrap())
        );
    }

    #[test]
    fn the_lowest_metric_wins_on_one_interface() {
        let routes = [
            route("eth0", "10.0.0.254", 200),
            route("eth0", "10.0.0.1", 100),
        ];
        assert_eq!(
            gateway_for(&routes, Some("eth0")).map(|r| r.gateway),
            Some("10.0.0.1".parse().unwrap())
        );
    }

    #[test]
    fn no_known_interface_or_no_route_on_it_means_no_gateway() {
        let routes = [route("eth0", "10.0.0.1", 100)];
        assert!(gateway_for(&routes, None).is_none());
        assert!(gateway_for(&routes, Some("wlan0")).is_none());
        assert!(gateway_for(&[], Some("eth0")).is_none());
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
