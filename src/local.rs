//! `myip local`: which addresses the interfaces carry, which of them leaves via the default
//! route, and through which gateway.
//!
//! Everything here is observed on this machine: the interface list comes from the operating
//! system, the egress address from asking the kernel to route a UDP socket (nothing is
//! sent), and the gateway from the Linux routing tables. Whatever cannot be determined is
//! `null`, never guessed.

use std::net::{IpAddr, SocketAddr};

use crate::Context;
use crate::cli::Options;
use crate::egress::{
    DefaultRoute, EgressProbe, PROBE_V4, PROBE_V6, gateway_for, parse_ipv4_routes,
    parse_ipv6_routes,
};
use crate::env::{Env, RecordingEnv};
use crate::fact::{Basis, SCHEMA_VERSION};
use crate::http::Family;
use crate::ifaces::{Interface, Interfaces, likely_virtual};
use crate::json::Json;
use crate::net::{Scope, classify};
use crate::output;
use crate::sanitize;

const ROUTES_V4: &str = "/proc/net/route";
const ROUTES_V6: &str = "/proc/net/ipv6_route";
const SOURCE_INTERFACES: &str = "os:network-interfaces";
const SOURCE_EGRESS: &str = "os:udp-connect";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInfo {
    pub ip: IpAddr,
    pub prefix_len: u8,
    pub scope: Scope,
    /// This is the address the operating system would use to reach the Internet.
    pub primary_egress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInfo {
    pub name: String,
    pub index: Option<u32>,
    /// A hint from the name alone; see [`crate::ifaces::likely_virtual`].
    pub likely_virtual: bool,
    pub addresses: Vec<AddressInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayInfo {
    pub address: IpAddr,
    pub scope: Scope,
}

/// The local address used to reach the Internet over one IP family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressInfo {
    pub address: IpAddr,
    pub scope: Scope,
    /// The interface that owns `address`; unknown when enumeration failed or no interface
    /// carries it.
    pub interface: Option<String>,
    pub gateway: Option<GatewayInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub family: Family,
    /// The interface list, or why it could not be read.
    pub interfaces: Result<Vec<InterfaceInfo>, String>,
    pub ipv4: Option<EgressInfo>,
    pub ipv6: Option<EgressInfo>,
    /// Details for `-v`: probes that failed, route tables that could not be read.
    pub notes: Vec<String>,
}

/// Looks at this machine through the given seams. `family` limits the addresses and the
/// egress probes to one IP family.
pub fn gather(
    env: &dyn Env,
    interfaces: &dyn Interfaces,
    probe: &dyn EgressProbe,
    family: Family,
) -> Report {
    let mut notes = Vec::new();
    let listed = interfaces
        .list()
        .map_err(|e| format!("cannot list network interfaces: {e}"));

    let ipv4 = if family == Family::V6 {
        None
    } else {
        egress(env, probe, listed.as_ref().ok(), V4, &mut notes)
    };
    let ipv6 = if family == Family::V4 {
        None
    } else {
        egress(env, probe, listed.as_ref().ok(), V6, &mut notes)
    };

    let interfaces = listed.map(|list| {
        list.into_iter()
            .filter_map(|interface| describe(interface, family, [&ipv4, &ipv6]))
            .collect()
    });
    Report {
        family,
        interfaces,
        ipv4,
        ipv6,
        notes,
    }
}

/// Prints the report. Exits 1 only when the interfaces could not be listed; the JSON
/// document is printed regardless, with `interfaces` and `error` saying why.
pub fn run(options: &Options, ctx: &mut Context<'_>) -> u8 {
    let env = RecordingEnv::new(ctx.host.env);
    let report = gather(&env, ctx.host.interfaces, ctx.host.probe, options.family());
    output::trace(options, &env, &report.notes, ctx.err);
    if let Some(error) = report.error() {
        output::diagnostic(ctx.err, error);
    }
    let text = report.error().is_none().then(|| report.render_text());
    let written = output::emit(options, &report.to_json(), text.as_deref(), ctx.out);
    output::exit_code(written, report.exit_code())
}

/// What differs between the two address families when looking for the egress.
struct Side {
    label: &'static str,
    target: SocketAddr,
    routes: &'static str,
    parse: fn(&str) -> Vec<DefaultRoute>,
    matches: fn(&IpAddr) -> bool,
}

const V4: Side = Side {
    label: "IPv4",
    target: PROBE_V4,
    routes: ROUTES_V4,
    parse: parse_ipv4_routes,
    matches: IpAddr::is_ipv4,
};

const V6: Side = Side {
    label: "IPv6",
    target: PROBE_V6,
    routes: ROUTES_V6,
    parse: parse_ipv6_routes,
    matches: IpAddr::is_ipv6,
};

fn egress(
    env: &dyn Env,
    probe: &dyn EgressProbe,
    listed: Option<&Vec<Interface>>,
    side: Side,
    notes: &mut Vec<String>,
) -> Option<EgressInfo> {
    let address = match probe.local_address_towards(side.target) {
        Ok(ip) if (side.matches)(&ip) && !ip.is_unspecified() => ip,
        Ok(ip) => {
            notes.push(format!(
                "{} egress probe answered {ip}, which is not a usable {} address",
                side.label, side.label
            ));
            return None;
        }
        Err(e) => {
            notes.push(format!("{} egress probe failed: {e}", side.label));
            return None;
        }
    };
    let interface = listed.and_then(|list| {
        list.iter()
            .find(|interface| interface.addresses.iter().any(|a| a.ip == address))
            .map(|interface| interface.name.clone())
    });
    let gateway = match env.read_to_string(side.routes) {
        Ok(content) => {
            let routes = (side.parse)(&content);
            let found = gateway_for(&routes, interface.as_deref(), &address);
            if found.is_none() && interface.is_some() {
                notes.push(format!(
                    "{}: no single default route in {} applies to {address}; gateway left unknown",
                    side.label, side.routes
                ));
            }
            found.map(|route| GatewayInfo {
                address: route.gateway,
                scope: classify(route.gateway),
            })
        }
        Err(e) => {
            notes.push(format!("cannot read {}: {e}", side.routes));
            None
        }
    };
    Some(EgressInfo {
        address,
        scope: classify(address),
        interface,
        gateway,
    })
}

/// Keeps the addresses of the requested family; an interface left with none is dropped.
fn describe(
    interface: Interface,
    family: Family,
    egresses: [&Option<EgressInfo>; 2],
) -> Option<InterfaceInfo> {
    let addresses: Vec<AddressInfo> = interface
        .addresses
        .iter()
        .filter(|a| match family {
            Family::Any => true,
            Family::V4 => a.ip.is_ipv4(),
            Family::V6 => a.ip.is_ipv6(),
        })
        .map(|a| AddressInfo {
            ip: a.ip,
            prefix_len: a.prefix_len,
            scope: classify(a.ip),
            primary_egress: egresses
                .iter()
                .any(|e| e.as_ref().is_some_and(|e| e.address == a.ip)),
        })
        .collect();
    if addresses.is_empty() && family != Family::Any {
        return None;
    }
    Some(InterfaceInfo {
        likely_virtual: likely_virtual(&interface.name),
        name: interface.name,
        index: interface.index,
        addresses,
    })
}

fn family_name(ip: &IpAddr) -> &'static str {
    if ip.is_ipv4() { "ipv4" } else { "ipv6" }
}

fn routes_source(ip: &IpAddr) -> String {
    format!("file:{}", if ip.is_ipv4() { ROUTES_V4 } else { ROUTES_V6 })
}

impl Report {
    /// `1` only when the interface list itself could not be read.
    pub fn exit_code(&self) -> u8 {
        u8::from(self.interfaces.is_err())
    }

    pub fn error(&self) -> Option<&str> {
        self.interfaces.as_ref().err().map(String::as_str)
    }

    pub fn to_json(&self) -> Json {
        let interfaces = match &self.interfaces {
            Ok(list) => Json::Array(list.iter().map(interface_json).collect()),
            Err(_) => Json::Null,
        };
        let mut egress = Vec::new();
        if self.family != Family::V6 {
            egress.push(("ipv4", egress_json(self.ipv4.as_ref())));
        }
        if self.family != Family::V4 {
            egress.push(("ipv6", egress_json(self.ipv6.as_ref())));
        }
        Json::object([
            ("schema_version", Json::from(SCHEMA_VERSION)),
            ("interfaces", interfaces),
            ("egress", Json::object(egress)),
            ("error", Json::from(self.error())),
        ])
    }

    pub fn render_text(&self) -> String {
        let mut out = String::from("interfaces (observed)\n");
        match &self.interfaces {
            Err(message) => out.push_str(&format!("  unavailable: {message}\n")),
            Ok(list) if list.is_empty() => out.push_str("  no interfaces found\n"),
            Ok(list) => render_interfaces(&mut out, list),
        }
        out.push_str(&format!(
            "egress (observed: a UDP connect() towards {}, which sends nothing)\n",
            self.probe_targets()
        ));
        for (label, egress) in self.egresses() {
            match egress {
                None => out.push_str(&format!(
                    "  {label}  unknown (the egress probe found no address; -v shows why)\n"
                )),
                Some(e) => out.push_str(&format!("  {label}  {}\n", egress_line(e))),
            }
        }
        if self
            .interfaces
            .as_ref()
            .is_ok_and(|list| list.iter().any(|i| i.likely_virtual))
        {
            out.push_str("\"likely virtual\" is a guess from the interface name, not a fact.\n");
        }
        out
    }

    fn egresses(&self) -> Vec<(&'static str, Option<&EgressInfo>)> {
        let mut rows = Vec::new();
        if self.family != Family::V6 {
            rows.push((V4.label, self.ipv4.as_ref()));
        }
        if self.family != Family::V4 {
            rows.push((V6.label, self.ipv6.as_ref()));
        }
        rows
    }

    fn probe_targets(&self) -> String {
        let mut targets = Vec::new();
        if self.family != Family::V6 {
            targets.push(PROBE_V4.ip().to_string());
        }
        if self.family != Family::V4 {
            targets.push(PROBE_V6.ip().to_string());
        }
        targets.join(" and ")
    }
}

fn interface_json(interface: &InterfaceInfo) -> Json {
    let addresses = interface
        .addresses
        .iter()
        .map(|a| {
            Json::object([
                ("ip", Json::from(a.ip.to_string())),
                ("family", Json::from(family_name(&a.ip))),
                ("prefix_len", Json::Int(i64::from(a.prefix_len))),
                ("scope", Json::from(a.scope.as_str())),
                ("primary_egress", Json::from(a.primary_egress)),
            ])
        })
        .collect();
    Json::object([
        ("name", Json::from(interface.name.as_str())),
        ("index", Json::from(interface.index)),
        ("likely_virtual", Json::from(interface.likely_virtual)),
        ("source", Json::from(SOURCE_INTERFACES)),
        ("basis", Json::from(Basis::Observed.as_str())),
        ("addresses", Json::Array(addresses)),
    ])
}

fn egress_json(egress: Option<&EgressInfo>) -> Json {
    let Some(egress) = egress else {
        return Json::Null;
    };
    let gateway = match &egress.gateway {
        None => Json::Null,
        Some(gateway) => Json::object([
            ("address", Json::from(gateway.address.to_string())),
            ("scope", Json::from(gateway.scope.as_str())),
            ("source", Json::from(routes_source(&gateway.address))),
            ("basis", Json::from(Basis::Observed.as_str())),
        ]),
    };
    Json::object([
        ("address", Json::from(egress.address.to_string())),
        ("scope", Json::from(egress.scope.as_str())),
        ("interface", Json::from(egress.interface.as_deref())),
        ("gateway", gateway),
        ("source", Json::from(SOURCE_EGRESS)),
        ("basis", Json::from(Basis::Observed.as_str())),
    ])
}

fn egress_line(egress: &EgressInfo) -> String {
    let mut line = format!("{} ({})", egress.address, egress.scope.as_str());
    match &egress.interface {
        Some(name) => line.push_str(&format!(" on {}", shown_name(name))),
        None => line.push_str(", interface unknown"),
    }
    match &egress.gateway {
        Some(gateway) => line.push_str(&format!(" via {}", gateway.address)),
        None => line.push_str(", gateway unknown"),
    }
    line
}

/// An interface name as shown in text: control characters are made visible, and a name
/// that is empty is called out instead of leaving a blank-looking line. JSON keeps the
/// name as it is.
fn shown_name(name: &str) -> String {
    if name.is_empty() {
        "(unnamed)".to_owned()
    } else {
        sanitize::escape(name)
    }
}

fn render_interfaces(out: &mut String, list: &[InterfaceInfo]) {
    let cidr = |a: &AddressInfo| format!("{}/{}", a.ip, a.prefix_len);
    let addr_width = list
        .iter()
        .flat_map(|i| &i.addresses)
        .map(|a| cidr(a).len())
        .max()
        .unwrap_or(0);
    let scope_width = list
        .iter()
        .flat_map(|i| &i.addresses)
        .map(|a| a.scope.as_str().len())
        .max()
        .unwrap_or(0);
    for interface in list {
        out.push_str(&format!("  {}", shown_name(&interface.name)));
        if interface.likely_virtual {
            out.push_str(" (likely virtual)");
        }
        out.push('\n');
        for address in &interface.addresses {
            let mut line = format!(
                "    {:<addr_width$}  {:<scope_width$}",
                cidr(address),
                address.scope.as_str()
            );
            if address.primary_egress {
                let family = if address.ip.is_ipv4() {
                    V4.label
                } else {
                    V6.label
                };
                line.push_str(&format!("  primary egress ({family})"));
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::egress::testing::FakeProbe;
    use crate::egress::{PROBE_V4, PROBE_V6};
    use crate::env::RecordingEnv;
    use crate::env::testing::FakeEnv;
    use crate::ifaces::testing::{FakeInterfaces, iface};

    const DOCKER_ROUTES: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        eth0\t00000000\t010011AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
        eth0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";

    const V6_ROUTES: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000a0027fffe7d0e5b 00000064 00000001 00000000 00450003 eth0\n";

    fn container() -> FakeInterfaces {
        FakeInterfaces::with(vec![
            iface("lo", 1, &[("127.0.0.1", 8), ("::1", 128)]),
            iface(
                "eth0",
                2,
                &[("172.17.0.2", 16), ("fe80::42:acff:fe11:2", 64)],
            ),
        ])
    }

    fn container_env() -> FakeEnv {
        FakeEnv::new().file(ROUTES_V4, DOCKER_ROUTES)
    }

    fn container_probe() -> FakeProbe {
        FakeProbe::new().route(PROBE_V4, "172.17.0.2")
    }

    fn mac() -> FakeInterfaces {
        FakeInterfaces::with(vec![
            iface("lo0", 1, &[("127.0.0.1", 8), ("::1", 128), ("fe80::1", 64)]),
            iface("en0", 6, &[("192.168.1.20", 24), ("fe80::1c", 64)]),
            iface("awdl0", 8, &[("fe80::a0", 64)]),
            iface("utun3", 9, &[("100.101.102.103", 32), ("fe80::3", 64)]),
        ])
    }

    fn mac_probe() -> FakeProbe {
        FakeProbe::new().route(PROBE_V4, "192.168.1.20")
    }

    fn json_at<'a>(json: &'a Json, path: &str) -> &'a Json {
        let mut current = json;
        for segment in path.split('.') {
            current = match current {
                Json::Object(fields) => fields
                    .iter()
                    .find(|(key, _)| key == segment)
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| panic!("no key {segment:?} on the way to {path}")),
                Json::Array(items) => {
                    let index: usize = segment.parse().unwrap();
                    items
                        .get(index)
                        .unwrap_or_else(|| panic!("no item {index} on the way to {path}"))
                }
                other => panic!("{path}: cannot step into {other:?} with {segment:?}"),
            };
        }
        current
    }

    fn str_at<'a>(json: &'a Json, path: &str) -> &'a str {
        match json_at(json, path) {
            Json::Str(text) => text,
            other => panic!("{path} is {other:?}, not a string"),
        }
    }

    fn keys(json: &Json) -> Vec<&str> {
        match json {
            Json::Object(fields) => fields.iter().map(|(key, _)| key.as_str()).collect(),
            other => panic!("not an object: {other:?}"),
        }
    }

    fn report(env: &FakeEnv, ifaces: &FakeInterfaces, probe: &FakeProbe, family: Family) -> Report {
        gather(env, ifaces, probe, family)
    }

    #[test]
    fn a_container_document_has_the_documented_shape_and_key_order() {
        let r = report(
            &container_env(),
            &container(),
            &container_probe(),
            Family::Any,
        );
        let expected = concat!(
            r#"{"schema_version":1,"interfaces":["#,
            r#"{"name":"lo","index":1,"likely_virtual":true,"source":"os:network-interfaces","basis":"observed","addresses":["#,
            r#"{"ip":"127.0.0.1","family":"ipv4","prefix_len":8,"scope":"loopback","primary_egress":false},"#,
            r#"{"ip":"::1","family":"ipv6","prefix_len":128,"scope":"loopback","primary_egress":false}]},"#,
            r#"{"name":"eth0","index":2,"likely_virtual":false,"source":"os:network-interfaces","basis":"observed","addresses":["#,
            r#"{"ip":"172.17.0.2","family":"ipv4","prefix_len":16,"scope":"private","primary_egress":true},"#,
            r#"{"ip":"fe80::42:acff:fe11:2","family":"ipv6","prefix_len":64,"scope":"link-local","primary_egress":false}]}],"#,
            r#""egress":{"ipv4":{"address":"172.17.0.2","scope":"private","interface":"eth0","#,
            r#""gateway":{"address":"172.17.0.1","scope":"private","source":"file:/proc/net/route","basis":"observed"},"#,
            r#""source":"os:udp-connect","basis":"observed"},"ipv6":null},"error":null}"#,
        );
        assert_eq!(r.to_json().render(false), expected);
        assert_eq!(r.exit_code(), 0);
        assert_eq!(r.error(), None);
    }

    #[test]
    fn every_address_carries_its_scope() {
        let ifaces = FakeInterfaces::with(vec![iface(
            "en0",
            1,
            &[
                ("192.168.1.20", 24),
                ("100.64.0.7", 10),
                ("169.254.3.4", 16),
                ("203.0.113.9", 24),
                ("8.8.4.4", 32),
                ("fd12:3456::1", 64),
                ("fe80::1", 64),
                ("2001:db8::5", 64),
                ("2606:4700::1", 64),
            ],
        )]);
        let r = report(&FakeEnv::new(), &ifaces, &FakeProbe::new(), Family::Any);
        let json = r.to_json();
        let scopes: Vec<&str> = (0..9)
            .map(|i| str_at(&json, &format!("interfaces.0.addresses.{i}.scope")))
            .collect();
        assert_eq!(
            scopes,
            [
                "private",
                "cgnat",
                "link-local",
                "documentation",
                "global",
                "unique-local",
                "link-local",
                "documentation",
                "global",
            ]
        );
    }

    #[test]
    fn virtual_looking_interfaces_are_flagged_and_physical_ones_are_not() {
        let r = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::Any);
        let flags: Vec<(String, bool)> = r
            .interfaces
            .unwrap()
            .into_iter()
            .map(|i| (i.name, i.likely_virtual))
            .collect();
        assert_eq!(
            flags,
            [
                ("lo0".to_owned(), true),
                ("en0".to_owned(), false),
                ("awdl0".to_owned(), true),
                ("utun3".to_owned(), true),
            ]
        );
    }

    #[test]
    fn only_the_egress_address_is_marked_primary() {
        let r = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::Any);
        let primary: Vec<String> = r
            .interfaces
            .unwrap()
            .iter()
            .flat_map(|i| {
                i.addresses
                    .iter()
                    .filter(|a| a.primary_egress)
                    .map(move |a| format!("{}:{}", i.name, a.ip))
            })
            .collect();
        assert_eq!(primary, ["en0:192.168.1.20"]);
    }

    #[test]
    fn each_family_has_its_own_primary_egress_address() {
        let ifaces = FakeInterfaces::with(vec![iface(
            "eth0",
            2,
            &[
                ("10.0.0.5", 24),
                ("2001:db8::5", 64),
                ("fe80::5", 64),
                ("2001:db8::6", 64),
            ],
        )]);
        let probe = FakeProbe::new()
            .route(PROBE_V4, "10.0.0.5")
            .route(PROBE_V6, "2001:db8::6");
        let r = report(&FakeEnv::new(), &ifaces, &probe, Family::Any);
        let primary: Vec<String> = r.interfaces.unwrap()[0]
            .addresses
            .iter()
            .filter(|a| a.primary_egress)
            .map(|a| a.ip.to_string())
            .collect();
        assert_eq!(primary, ["10.0.0.5", "2001:db8::6"]);
        assert_eq!(r.ipv4.unwrap().address.to_string(), "10.0.0.5");
        assert_eq!(r.ipv6.unwrap().address.to_string(), "2001:db8::6");
    }

    #[test]
    fn the_egress_names_its_interface_and_the_gateway_from_the_route_table() {
        let r = report(
            &container_env(),
            &container(),
            &container_probe(),
            Family::Any,
        );
        let egress = r.ipv4.unwrap();
        assert_eq!(egress.interface.as_deref(), Some("eth0"));
        let gateway = egress.gateway.unwrap();
        assert_eq!(gateway.address.to_string(), "172.17.0.1");
        assert_eq!(gateway.scope, Scope::Private);
    }

    #[test]
    fn an_ipv6_gateway_is_the_link_local_router_from_ipv6_route() {
        let env = FakeEnv::new().file(ROUTES_V6, V6_ROUTES);
        let ifaces = FakeInterfaces::with(vec![iface(
            "eth0",
            2,
            &[("2001:db8::5", 64), ("fe80::5", 64)],
        )]);
        let probe = FakeProbe::new().route(PROBE_V6, "2001:db8::5");
        let r = report(&env, &ifaces, &probe, Family::Any);
        let json = r.to_json();
        assert_eq!(
            str_at(&json, "egress.ipv6.gateway.address"),
            "fe80::a00:27ff:fe7d:e5b"
        );
        assert_eq!(str_at(&json, "egress.ipv6.gateway.scope"), "link-local");
        assert_eq!(
            str_at(&json, "egress.ipv6.gateway.source"),
            "file:/proc/net/ipv6_route"
        );
        assert_eq!(json_at(&json, "egress.ipv4"), &Json::Null);
    }

    #[test]
    fn without_a_readable_route_table_the_gateway_is_null_and_the_run_still_succeeds() {
        let r = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::Any);
        let egress = r.ipv4.as_ref().unwrap();
        assert_eq!(egress.interface.as_deref(), Some("en0"));
        assert!(egress.gateway.is_none());
        assert_eq!(r.exit_code(), 0);
        assert!(
            r.notes.iter().any(|n| n.contains("/proc/net/route")),
            "{:?}",
            r.notes
        );
        assert_eq!(json_at(&r.to_json(), "egress.ipv4.gateway"), &Json::Null);
    }

    /// One `/proc/net/ipv6_route` line for a default route; `source` is the source-specific
    /// constraint (prefix, length), `None` for an unconditional default.
    fn v6_default(
        source: Option<(&str, u8)>,
        gateway: &str,
        metric: u32,
        interface: &str,
    ) -> String {
        let bits = |text: &str| text.parse::<std::net::Ipv6Addr>().unwrap().to_bits();
        let (source, length) = source.map_or((0, 0), |(prefix, length)| (bits(prefix), length));
        format!(
            "{:032x} 00 {source:032x} {length:02x} {:032x} {metric:08x} 00000001 00000000 00450003 {interface}\n",
            0u128,
            bits(gateway),
        )
    }

    fn v6_egress_from(table: &str, address: &str) -> (Report, Option<String>) {
        let env = FakeEnv::new().file(ROUTES_V6, table);
        let ifaces =
            FakeInterfaces::with(vec![iface("eth0", 2, &[(address, 64), ("fe80::5", 64)])]);
        let probe = FakeProbe::new().route(PROBE_V6, address);
        let r = report(&env, &ifaces, &probe, Family::V6);
        let gateway = r
            .ipv6
            .as_ref()
            .and_then(|e| e.gateway.as_ref())
            .map(|g| g.address.to_string());
        (r, gateway)
    }

    #[test]
    fn a_source_specific_ipv6_default_is_matched_against_the_egress_address() {
        let table = [
            v6_default(Some(("2001:db8:1::", 64)), "fe80::1", 100, "eth0"),
            v6_default(Some(("2001:db8:2::", 64)), "fe80::2", 100, "eth0"),
        ]
        .concat();
        let (_, gateway) = v6_egress_from(&table, "2001:db8:2::5");
        assert_eq!(gateway.as_deref(), Some("fe80::2"));
        let (_, gateway) = v6_egress_from(&table, "2001:db8:1::5");
        assert_eq!(gateway.as_deref(), Some("fe80::1"));
    }

    #[test]
    fn no_applicable_source_specific_route_means_a_null_gateway_never_a_guess() {
        let table = v6_default(Some(("2001:db8:1::", 64)), "fe80::1", 100, "eth0");
        let (r, gateway) = v6_egress_from(&table, "2001:db8:2::5");
        assert_eq!(gateway, None);
        assert_eq!(json_at(&r.to_json(), "egress.ipv6.gateway"), &Json::Null);
        assert!(r.render_text().contains("gateway unknown"));
        assert!(
            r.notes.iter().any(|n| n.contains("ipv6_route")),
            "{:?}",
            r.notes
        );
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn a_source_specific_default_beats_an_unconditional_one_with_a_lower_metric() {
        let table = [
            v6_default(None, "fe80::a", 50, "eth0"),
            v6_default(Some(("2001:db8:2::", 64)), "fe80::2", 100, "eth0"),
        ]
        .concat();
        let (_, specific) = v6_egress_from(&table, "2001:db8:2::5");
        assert_eq!(specific.as_deref(), Some("fe80::2"));
        let (_, other) = v6_egress_from(&table, "2001:db8:3::5");
        assert_eq!(other.as_deref(), Some("fe80::a"));
    }

    #[test]
    fn equally_good_defaults_with_different_gateways_are_ambiguous_so_null() {
        let table = [
            v6_default(None, "fe80::a", 100, "eth0"),
            v6_default(None, "fe80::b", 100, "eth0"),
        ]
        .concat();
        let (_, gateway) = v6_egress_from(&table, "2001:db8:2::5");
        assert_eq!(gateway, None);
        let same = [
            v6_default(None, "fe80::a", 100, "eth0"),
            v6_default(None, "fe80::a", 100, "eth0"),
        ]
        .concat();
        let (_, gateway) = v6_egress_from(&same, "2001:db8:2::5");
        assert_eq!(gateway.as_deref(), Some("fe80::a"));
    }

    #[test]
    fn a_missing_egress_result_is_unknown_not_no_route() {
        let text = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::Any).render_text();
        let line = text
            .lines()
            .find(|l| l.contains("IPv6  "))
            .unwrap_or_else(|| panic!("no IPv6 row in:\n{text}"));
        assert_eq!(
            line.trim(),
            "IPv6  unknown (the egress probe found no address; -v shows why)"
        );
        assert!(!text.contains("no route"), "{text}");
    }

    #[test]
    fn an_interface_without_a_name_is_shown_as_unnamed() {
        let ifaces = FakeInterfaces::with(vec![iface("", 3, &[("10.0.0.1", 8)])]);
        let probe = FakeProbe::new().route(PROBE_V4, "10.0.0.1");
        let r = report(&FakeEnv::new(), &ifaces, &probe, Family::V4);
        let text = r.render_text();
        assert!(text.contains("(unnamed)"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.trim().starts_with("IPv4  10.0.0.1") && l.contains("on (unnamed)")),
            "{text}"
        );
        assert_eq!(str_at(&r.to_json(), "interfaces.0.name"), "");
    }

    #[test]
    fn hostile_interface_names_are_shown_visibly_in_text_and_stay_json_escaped() {
        let name = "eth\u{1b}[2J0";
        let ifaces = FakeInterfaces::with(vec![iface(name, 2, &[("10.0.0.1", 8)])]);
        let probe = FakeProbe::new().route(PROBE_V4, "10.0.0.1");
        let r = report(&FakeEnv::new(), &ifaces, &probe, Family::V4);
        let text = r.render_text();
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(text.contains("eth\\x1b[2J0"), "{text}");
        let json = r.to_json().render(false);
        assert!(!json.contains('\u{1b}'), "{json:?}");
        assert!(json.contains("eth\\u001b[2J0"), "{json}");
    }

    #[test]
    fn a_default_route_on_another_interface_is_not_this_egress_gateway() {
        let env = FakeEnv::new().file(
            ROUTES_V4,
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
             eth1\t00000000\t0100000A\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
        );
        let r = report(&env, &container(), &container_probe(), Family::Any);
        assert!(r.ipv4.unwrap().gateway.is_none());
    }

    #[test]
    fn a_family_without_a_route_is_null_while_its_addresses_are_still_listed() {
        let r = report(
            &container_env(),
            &container(),
            &container_probe(),
            Family::Any,
        );
        assert!(r.ipv6.is_none());
        assert_eq!(json_at(&r.to_json(), "egress.ipv6"), &Json::Null);
        let addresses = &r.interfaces.as_ref().unwrap()[1].addresses;
        assert!(addresses.iter().any(|a| a.ip.is_ipv6()));
        assert!(
            r.notes.iter().any(|n| n.contains("IPv6")),
            "the failed probe is explained for -v: {:?}",
            r.notes
        );
    }

    #[test]
    fn an_offline_machine_still_lists_interfaces_and_exits_0() {
        let r = report(&FakeEnv::new(), &mac(), &FakeProbe::new(), Family::Any);
        assert!(r.ipv4.is_none() && r.ipv6.is_none());
        assert_eq!(r.exit_code(), 0);
        assert_eq!(r.interfaces.unwrap().len(), 4);
    }

    #[test]
    fn a_probe_answer_of_the_wrong_family_or_unspecified_is_not_an_egress() {
        let probe = FakeProbe::new()
            .route(PROBE_V4, "::1")
            .route(PROBE_V6, "0.0.0.0");
        let r = report(&FakeEnv::new(), &mac(), &probe, Family::Any);
        assert!(r.ipv4.is_none());
        assert!(r.ipv6.is_none());
        let unspecified = FakeProbe::new().route(PROBE_V4, "0.0.0.0");
        assert!(
            report(&FakeEnv::new(), &mac(), &unspecified, Family::Any)
                .ipv4
                .is_none()
        );
    }

    #[test]
    fn an_egress_address_no_interface_carries_is_kept_without_inventing_an_owner() {
        let probe = FakeProbe::new().route(PROBE_V4, "10.99.99.99");
        let r = report(&container_env(), &container(), &probe, Family::Any);
        let egress = r.ipv4.unwrap();
        assert_eq!(egress.address.to_string(), "10.99.99.99");
        assert_eq!(egress.interface, None);
        assert!(egress.gateway.is_none());
        assert!(
            r.interfaces
                .unwrap()
                .iter()
                .flat_map(|i| &i.addresses)
                .all(|a| !a.primary_egress)
        );
    }

    #[test]
    fn dash_4_lists_only_ipv4_and_never_asks_about_ipv6() {
        let probe = container_probe().route(PROBE_V6, "2001:db8::5");
        let r = report(&container_env(), &container(), &probe, Family::V4);
        assert_eq!(probe.asked(), vec![PROBE_V4]);
        let json = r.to_json();
        assert_eq!(keys(json_at(&json, "egress")), ["ipv4"]);
        let families: Vec<&str> = r
            .interfaces
            .as_ref()
            .unwrap()
            .iter()
            .flat_map(|i| &i.addresses)
            .map(|a| if a.ip.is_ipv4() { "ipv4" } else { "ipv6" })
            .collect();
        assert!(families.iter().all(|f| *f == "ipv4"), "{families:?}");
    }

    #[test]
    fn dash_6_lists_only_ipv6_and_drops_interfaces_left_without_addresses() {
        let ifaces = FakeInterfaces::with(vec![
            iface("eth0", 2, &[("10.0.0.5", 24)]),
            iface("eth1", 3, &[("10.0.1.5", 24), ("2001:db8::7", 64)]),
        ]);
        let probe = FakeProbe::new()
            .route(PROBE_V4, "10.0.0.5")
            .route(PROBE_V6, "2001:db8::7");
        let r = report(&FakeEnv::new(), &ifaces, &probe, Family::V6);
        assert_eq!(probe.asked(), vec![PROBE_V6]);
        let interfaces = r.interfaces.as_ref().unwrap();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "eth1");
        assert_eq!(interfaces[0].addresses.len(), 1);
        assert_eq!(keys(json_at(&r.to_json(), "egress")), ["ipv6"]);
    }

    #[test]
    fn dash_4_reads_only_the_ipv4_route_table() {
        let env = container_env();
        let spy = RecordingEnv::new(&env);
        gather(&spy, &container(), &container_probe(), Family::V4);
        let seen: Vec<String> = spy.accesses().iter().map(ToString::to_string).collect();
        assert_eq!(seen, ["read /proc/net/route"]);
    }

    #[test]
    fn only_the_two_route_tables_are_ever_read() {
        let env = container_env();
        let spy = RecordingEnv::new(&env);
        let probe = container_probe().route(PROBE_V6, "2001:db8::5");
        gather(&spy, &container(), &probe, Family::Any);
        let seen: Vec<String> = spy.accesses().iter().map(ToString::to_string).collect();
        assert_eq!(seen, ["read /proc/net/route", "read /proc/net/ipv6_route"]);
    }

    #[test]
    fn a_failed_enumeration_exits_1_and_the_json_still_parses_with_an_error() {
        let ifaces = FakeInterfaces::failing(io::ErrorKind::PermissionDenied);
        let r = report(&container_env(), &ifaces, &container_probe(), Family::Any);
        assert_eq!(r.exit_code(), 1);
        let message = r.error().unwrap();
        assert!(message.contains("network interfaces"), "{message}");
        let json = r.to_json();
        assert_eq!(json_at(&json, "interfaces"), &Json::Null);
        assert_eq!(str_at(&json, "error"), message);
        assert_eq!(json_at(&json, "schema_version"), &Json::Int(1));
    }

    #[test]
    fn a_failed_enumeration_still_reports_the_egress_it_could_observe_without_an_owner() {
        let ifaces = FakeInterfaces::failing(io::ErrorKind::PermissionDenied);
        let r = report(&container_env(), &ifaces, &container_probe(), Family::Any);
        let egress = r.ipv4.unwrap();
        assert_eq!(egress.address.to_string(), "172.17.0.2");
        assert_eq!(egress.interface, None);
        assert!(egress.gateway.is_none());
    }

    #[test]
    fn the_text_marks_primary_egress_virtual_hints_scopes_and_the_gateway() {
        let env = FakeEnv::new().file(
            ROUTES_V4,
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
             en0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
        );
        let text = report(&env, &mac(), &mac_probe(), Family::Any).render_text();
        let line = |needle: &str| {
            text.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
        };
        let primary = line("192.168.1.20/24");
        assert!(primary.contains("private"), "{primary}");
        assert!(primary.contains("primary egress (IPv4)"), "{primary}");
        assert!(line("utun3").contains("likely virtual"));
        assert!(!line("en0").contains("likely virtual"));
        assert!(line("100.101.102.103/32").contains("cgnat"));
        assert!(line("fe80::1c/64").contains("link-local"));
        assert_eq!(
            line("IPv4  192").trim(),
            "IPv4  192.168.1.20 (private) on en0 via 192.168.1.1"
        );
        assert_eq!(
            line("IPv6  ").trim(),
            "IPv6  unknown (the egress probe found no address; -v shows why)"
        );
        assert!(text.contains("observed"), "{text}");
    }

    #[test]
    fn the_text_says_when_the_gateway_is_unknown() {
        let text = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::Any).render_text();
        assert!(text.contains("gateway unknown"), "{text}");
    }

    #[test]
    fn the_text_for_dash_4_does_not_mention_ipv6_egress() {
        let text = report(&FakeEnv::new(), &mac(), &mac_probe(), Family::V4).render_text();
        assert!(text.contains("IPv4  192.168.1.20"), "{text}");
        assert!(!text.contains("IPv6  "), "{text}");
    }

    #[test]
    fn an_empty_interface_list_is_a_valid_answer() {
        let ifaces = FakeInterfaces::with(Vec::new());
        let r = report(&FakeEnv::new(), &ifaces, &FakeProbe::new(), Family::Any);
        assert_eq!(r.exit_code(), 0);
        assert_eq!(
            json_at(&r.to_json(), "interfaces"),
            &Json::Array(Vec::new())
        );
        assert!(r.render_text().contains("no interfaces"));
    }
}
