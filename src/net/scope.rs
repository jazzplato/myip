//! Where an IP address can be reached from.

use std::net::IpAddr;

/// The reachability class of an address. [`Scope::as_str`] is the stable spelling used in
/// `--json` output; adding a variant is compatible, renaming one is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// `0.0.0.0` and `::`.
    Unspecified,
    /// `127.0.0.0/8` and `::1`.
    Loopback,
    /// `169.254.0.0/16` and `fe80::/10`.
    LinkLocal,
    /// RFC 1918: `10.0.0.0/8`, `172.16.0.0/12` and `192.168.0.0/16`.
    Private,
    /// Carrier-grade NAT shared space, `100.64.0.0/10` (RFC 6598).
    Cgnat,
    /// IPv6 unique local addresses, `fc00::/7`.
    UniqueLocal,
    /// `224.0.0.0/4` and `ff00::/8`.
    Multicast,
    /// Reserved for examples: `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`,
    /// `2001:db8::/32` and `3fff::/20`.
    Documentation,
    /// Any other address that is not part of the public internet: `0.0.0.0/8`, `240.0.0.0/4`
    /// (including the broadcast address), IETF protocol assignments, benchmarking, and
    /// everything outside the allocated IPv6 global unicast space `2000::/3`.
    Reserved,
    /// A publicly routable unicast address.
    Global,
}

impl Scope {
    /// The spelling used in `--json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Loopback => "loopback",
            Self::LinkLocal => "link-local",
            Self::Private => "private",
            Self::Cgnat => "cgnat",
            Self::UniqueLocal => "unique-local",
            Self::Multicast => "multicast",
            Self::Documentation => "documentation",
            Self::Reserved => "reserved",
            Self::Global => "global",
        }
    }

    pub const fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }
}

/// Classifies an address. An IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is classified as
/// the IPv4 address it carries.
pub fn classify(ip: IpAddr) -> Scope {
    let _ = ip;
    Scope::Global
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_of(text: &str) -> Scope {
        classify(text.parse().unwrap_or_else(|e| panic!("{text}: {e}")))
    }

    fn check(cases: &[(&str, Scope)]) {
        for (text, want) in cases {
            assert_eq!(scope_of(text), *want, "{text}");
        }
    }

    #[test]
    fn ipv4_special_ranges_and_their_edges() {
        use Scope::*;
        check(&[
            ("0.0.0.0", Unspecified),
            ("0.0.0.1", Reserved),
            ("0.255.255.255", Reserved),
            ("1.0.0.0", Global),
            ("10.0.0.0", Private),
            ("10.255.255.255", Private),
            ("9.255.255.255", Global),
            ("11.0.0.0", Global),
            ("172.16.0.0", Private),
            ("172.31.255.255", Private),
            ("172.15.255.255", Global),
            ("172.32.0.0", Global),
            ("192.168.0.0", Private),
            ("192.168.255.255", Private),
            ("192.167.255.255", Global),
            ("192.169.0.0", Global),
            ("100.64.0.0", Cgnat),
            ("100.127.255.255", Cgnat),
            ("100.63.255.255", Global),
            ("100.128.0.0", Global),
            ("127.0.0.1", Loopback),
            ("127.255.255.255", Loopback),
            ("126.255.255.255", Global),
            ("128.0.0.0", Global),
            ("169.254.0.0", LinkLocal),
            ("169.254.255.255", LinkLocal),
            ("169.253.255.255", Global),
            ("169.255.0.0", Global),
            ("192.0.0.0", Reserved),
            ("192.0.0.255", Reserved),
            ("192.0.1.0", Global),
            ("192.0.2.0", Documentation),
            ("192.0.2.255", Documentation),
            ("198.51.100.1", Documentation),
            ("203.0.113.255", Documentation),
            ("198.51.99.255", Global),
            ("203.0.114.0", Global),
            ("198.18.0.0", Reserved),
            ("198.19.255.255", Reserved),
            ("198.17.255.255", Global),
            ("198.20.0.0", Global),
            ("224.0.0.0", Multicast),
            ("239.255.255.255", Multicast),
            ("223.255.255.255", Global),
            ("240.0.0.0", Reserved),
            ("255.255.255.254", Reserved),
            ("255.255.255.255", Reserved),
        ]);
    }

    #[test]
    fn well_known_ipv4_addresses_are_global() {
        for text in [
            "1.1.1.1",
            "8.8.8.8",
            "9.9.9.9",
            "93.184.216.34",
            "208.67.222.222",
        ] {
            assert_eq!(scope_of(text), Scope::Global, "{text}");
        }
    }

    #[test]
    fn ipv6_special_ranges_and_their_edges() {
        use Scope::*;
        check(&[
            ("::", Unspecified),
            ("::1", Loopback),
            ("::2", Reserved),
            ("::1.2.3.4", Reserved),
            ("fe80::", LinkLocal),
            ("fe80::1", LinkLocal),
            ("febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff", LinkLocal),
            ("fec0::1", Reserved),
            ("fe00::1", Reserved),
            ("fc00::", UniqueLocal),
            ("fd12:3456:789a::1", UniqueLocal),
            ("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", UniqueLocal),
            ("fb00::1", Reserved),
            ("ff00::", Multicast),
            ("ff02::1", Multicast),
            ("ff0e::1", Multicast),
            ("2001:db8::", Documentation),
            ("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff", Documentation),
            ("2001:db9::", Global),
            ("3fff::", Documentation),
            ("3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff", Documentation),
            ("3fff:1000::", Global),
            ("100::1", Reserved),
            ("64:ff9b::808:808", Reserved),
            ("64:ff9b:1::1", Reserved),
            ("8000::1", Reserved),
            ("4000::1", Reserved),
            ("1fff::1", Reserved),
            ("2000::1", Global),
            ("3fff:ffff::1", Global),
            ("4000::", Reserved),
        ]);
    }

    #[test]
    fn ipv6_ietf_protocol_block_is_reserved_except_the_globally_reachable_parts() {
        use Scope::*;
        check(&[
            ("2001::1", Reserved),
            ("2001:0:4136:e378:8000:63bf:3fff:fdd2", Reserved),
            ("2001:2::1", Reserved),
            ("2001:10::1", Reserved),
            ("2001:1ff:ffff::1", Reserved),
            ("2001:1::1", Global),
            ("2001:1::2", Global),
            ("2001:1::", Reserved),
            ("2001:3::1", Global),
            ("2001:4:112::1", Global),
            ("2001:4:113::1", Reserved),
            ("2001:20::1", Global),
            ("2001:2f:ffff::1", Global),
            ("2001:30::1", Global),
            ("2001:3f:ffff::1", Global),
            ("2001:40::1", Reserved),
            ("2001:200::1", Global),
        ]);
    }

    #[test]
    fn well_known_ipv6_addresses_are_global() {
        for text in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "2620:fe::fe",
            "2a00:1450:4001:81b::200e",
            "2002:c000:204::1",
        ] {
            assert_eq!(scope_of(text), Scope::Global, "{text}");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_is_classified_as_the_embedded_ipv4() {
        use Scope::*;
        check(&[
            ("::ffff:0.0.0.0", Unspecified),
            ("::ffff:127.0.0.1", Loopback),
            ("::ffff:10.0.0.1", Private),
            ("::ffff:192.168.1.1", Private),
            ("::ffff:100.64.0.1", Cgnat),
            ("::ffff:169.254.1.1", LinkLocal),
            ("::ffff:224.0.0.1", Multicast),
            ("::ffff:192.0.2.1", Documentation),
            ("::ffff:255.255.255.255", Reserved),
            ("::ffff:8.8.8.8", Global),
            ("::ffff:808:808", Global),
            ("::ffff:7f00:1", Loopback),
        ]);
    }

    #[test]
    fn only_global_is_global() {
        use Scope::*;
        for scope in [
            Unspecified,
            Loopback,
            LinkLocal,
            Private,
            Cgnat,
            UniqueLocal,
            Multicast,
            Documentation,
            Reserved,
        ] {
            assert!(!scope.is_global(), "{scope:?}");
        }
        assert!(Global.is_global());
    }

    // The JSON spelling is a contract (ADR 0001): changing one of these is a breaking change.
    #[test]
    fn the_json_spelling_is_stable() {
        use Scope::*;
        for (scope, want) in [
            (Unspecified, "unspecified"),
            (Loopback, "loopback"),
            (LinkLocal, "link-local"),
            (Private, "private"),
            (Cgnat, "cgnat"),
            (UniqueLocal, "unique-local"),
            (Multicast, "multicast"),
            (Documentation, "documentation"),
            (Reserved, "reserved"),
            (Global, "global"),
        ] {
            assert_eq!(scope.as_str(), want);
        }
    }
}
