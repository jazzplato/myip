//! `myip all`: what the internet sees (`public`), which addresses this machine has (`local`)
//! and where it runs (`where`), side by side, with the two comparisons people actually want:
//! does the egress address differ from the public one (NAT), and for a pod, which address is
//! the pod's, the node's and the internet's.
//!
//! It is a report, not a gate: a section that fails is reported in-band and never makes the
//! run exit 1.

use std::net::IpAddr;

use crate::egress::EgressProbe;
use crate::env::Env;
use crate::http::Family;
use crate::ifaces::Interfaces;
use crate::json::Json;
use crate::local::Report;
use crate::location::Location;

/// The internet's view of this machine, as `all` needs it. The `public` module fills it in;
/// the seam keeps this module testable without a network.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicSection {
    /// The public address, when a provider gave one.
    pub ip: Option<IpAddr>,
    pub provider: Option<String>,
    /// Why there is no `ip`.
    pub error: Option<String>,
    /// `--verify` found providers disagreeing: `all` then exits 3.
    pub disagree: bool,
    /// The section as the `public` command renders it.
    pub json: Json,
}

impl PublicSection {
    /// A section for a lookup that produced no answer.
    pub fn unavailable(_error: &str) -> Self {
        unimplemented!()
    }
}

#[derive(Debug)]
pub struct Overview {
    pub public: PublicSection,
    pub local: Report,
    pub location: Location,
}

/// Looks at this machine through the given seams and combines it with the public answer.
/// `family` limits the `local` section, like `-4`/`-6`; `where` ignores it.
pub fn gather(
    _env: &dyn Env,
    _interfaces: &dyn Interfaces,
    _probe: &dyn EgressProbe,
    _family: Family,
    _public: PublicSection,
) -> Overview {
    unimplemented!()
}

impl Overview {
    /// `0`, or `3` when `--verify` found the providers disagreeing. Never `1`.
    pub fn exit_code(&self) -> u8 {
        unimplemented!()
    }

    pub fn to_json(&self) -> Json {
        unimplemented!()
    }

    pub fn render_text(&self) -> String {
        unimplemented!()
    }

    /// One line per section that could not be produced, for stderr.
    pub fn notes(&self) -> Vec<String> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::testing::FakeProbe;
    use crate::egress::{PROBE_V4, PROBE_V6};
    use crate::env::testing::FakeEnv;
    use crate::ifaces::testing::{FakeInterfaces, iface};

    fn lan() -> FakeInterfaces {
        FakeInterfaces::with(vec![
            iface("lo", 1, &[("127.0.0.1", 8)]),
            iface("en0", 2, &[("192.168.1.23", 24), ("fd00::5", 64)]),
        ])
    }

    fn v4_only() -> FakeProbe {
        FakeProbe::new().route(PROBE_V4, "192.168.1.23")
    }

    fn dual_stack() -> FakeProbe {
        v4_only().route(PROBE_V6, "fd00::5")
    }

    fn answered(ip: &str) -> PublicSection {
        PublicSection {
            ip: Some(ip.parse().unwrap()),
            provider: Some("ipify".to_owned()),
            error: None,
            disagree: false,
            json: Json::object([
                ("schema_version", 1u32.into()),
                ("ip", ip.into()),
                ("provider", "ipify".into()),
            ]),
        }
    }

    fn pod() -> FakeEnv {
        FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_IP", "10.244.1.7")
            .var("HOST_IP", "192.168.49.2")
    }

    fn overview(env: &FakeEnv, probe: &FakeProbe, public: PublicSection) -> Overview {
        gather(env, &lan(), probe, Family::Any, public)
    }

    fn at<'a>(json: &'a Json, path: &[&str]) -> &'a Json {
        path.iter().fold(json, |node, key| match node {
            Json::Object(fields) => {
                &fields
                    .iter()
                    .find(|(name, _)| name == key)
                    .unwrap_or_else(|| panic!("no key {key:?} in {node:?}"))
                    .1
            }
            other => panic!("{other:?} has no key {key:?}"),
        })
    }

    fn keys(json: &Json) -> Vec<&str> {
        match json {
            Json::Object(fields) => fields.iter().map(|(name, _)| name.as_str()).collect(),
            other => panic!("{other:?} is not an object"),
        }
    }

    fn s(text: &str) -> Json {
        Json::Str(text.to_owned())
    }

    #[test]
    fn the_document_has_the_schema_version_then_the_three_sections_then_highlights() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).to_json();
        assert_eq!(
            keys(&json),
            ["schema_version", "public", "local", "where", "highlights"]
        );
        assert_eq!(at(&json, &["schema_version"]), &Json::Int(1));
    }

    #[test]
    fn sections_do_not_repeat_the_schema_version() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).to_json();
        for section in ["public", "local", "where"] {
            assert!(
                !keys(at(&json, &[section])).contains(&"schema_version"),
                "{section}"
            );
        }
        assert_eq!(at(&json, &["public", "ip"]), &s("203.0.113.7"));
    }

    #[test]
    fn the_local_and_where_sections_are_the_standalone_reports() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).to_json();
        assert_eq!(
            at(&json, &["local", "egress", "ipv4", "address"]),
            &s("192.168.1.23")
        );
        assert_eq!(at(&json, &["where", "environment"]), &s("host"));
    }

    #[test]
    fn nat_is_reported_when_the_egress_address_differs_from_the_public_one() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).to_json();
        let nat = at(&json, &["highlights", "nat"]);
        assert_eq!(at(nat, &["family"]), &s("ipv4"));
        assert_eq!(at(nat, &["local"]), &s("192.168.1.23"));
        assert_eq!(at(nat, &["public"]), &s("203.0.113.7"));
        assert_eq!(at(nat, &["translated"]), &Json::Bool(true));
    }

    #[test]
    fn no_nat_when_the_egress_address_is_the_public_one() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("192.168.1.23")).to_json();
        assert_eq!(
            at(&json, &["highlights", "nat", "translated"]),
            &Json::Bool(false)
        );
    }

    #[test]
    fn nat_follows_the_family_of_the_public_answer() {
        let json = overview(&FakeEnv::new(), &dual_stack(), answered("2606:4700::1")).to_json();
        let nat = at(&json, &["highlights", "nat"]);
        assert_eq!(at(nat, &["family"]), &s("ipv6"));
        assert_eq!(at(nat, &["local"]), &s("fd00::5"));
        assert_eq!(at(nat, &["translated"]), &Json::Bool(true));
    }

    #[test]
    fn nat_is_null_without_a_public_answer() {
        let public = PublicSection::unavailable("no provider answered");
        let json = overview(&FakeEnv::new(), &v4_only(), public).to_json();
        assert_eq!(at(&json, &["highlights", "nat"]), &Json::Null);
    }

    #[test]
    fn nat_is_null_without_a_local_egress_address_in_the_public_family() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("2606:4700::1")).to_json();
        assert_eq!(at(&json, &["highlights", "nat"]), &Json::Null);
    }

    #[test]
    fn a_pod_gets_its_pod_node_egress_and_public_addresses_side_by_side() {
        let probe = FakeProbe::new().route(PROBE_V4, "10.244.1.7");
        let json = overview(&pod(), &probe, answered("203.0.113.7")).to_json();
        let k8s = at(&json, &["highlights", "kubernetes"]);
        assert_eq!(at(k8s, &["pod_ip"]), &s("10.244.1.7"));
        assert_eq!(at(k8s, &["node_ip"]), &s("192.168.49.2"));
        assert_eq!(at(k8s, &["egress_ipv4"]), &s("10.244.1.7"));
        assert_eq!(at(k8s, &["egress_ipv6"]), &Json::Null);
        assert_eq!(at(k8s, &["public_ip"]), &s("203.0.113.7"));
    }

    #[test]
    fn the_kubernetes_highlight_is_null_outside_a_pod() {
        let json = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).to_json();
        assert_eq!(at(&json, &["highlights", "kubernetes"]), &Json::Null);
    }

    #[test]
    fn a_pod_without_the_downward_api_has_null_pod_and_node_addresses() {
        let env = FakeEnv::new().var("KUBERNETES_SERVICE_HOST", "10.96.0.1");
        let json = overview(&env, &v4_only(), answered("203.0.113.7")).to_json();
        let k8s = at(&json, &["highlights", "kubernetes"]);
        assert_eq!(at(k8s, &["pod_ip"]), &Json::Null);
        assert_eq!(at(k8s, &["node_ip"]), &Json::Null);
    }

    #[test]
    fn a_failed_public_lookup_is_in_band_and_does_not_fail_the_run() {
        let public = PublicSection::unavailable("no provider returned a valid public address");
        let all = overview(&FakeEnv::new(), &v4_only(), public);
        assert_eq!(all.exit_code(), 0);
        let json = all.to_json();
        assert_eq!(at(&json, &["public", "ip"]), &Json::Null);
        assert_eq!(
            at(&json, &["public", "error"]),
            &s("no provider returned a valid public address")
        );
        assert_eq!(
            all.notes(),
            ["public: no provider returned a valid public address"]
        );
        assert!(all.render_text().contains("unavailable"));
    }

    #[test]
    fn a_failed_interface_listing_is_in_band_and_does_not_fail_the_run() {
        let failing = FakeInterfaces::failing(std::io::ErrorKind::PermissionDenied);
        let all = gather(
            &FakeEnv::new(),
            &failing,
            &v4_only(),
            Family::Any,
            answered("203.0.113.7"),
        );
        assert_eq!(all.exit_code(), 0, "all never exits 1");
        let json = all.to_json();
        assert_eq!(at(&json, &["local", "interfaces"]), &Json::Null);
        assert!(matches!(at(&json, &["local", "error"]), Json::Str(_)));
        assert_eq!(at(&json, &["where", "environment"]), &s("host"));
        let notes = all.notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("local: cannot list network interfaces"));
    }

    #[test]
    fn a_verify_disagreement_exits_3_whatever_else_failed() {
        let mut public = answered("203.0.113.7");
        public.disagree = true;
        let failing = FakeInterfaces::failing(std::io::ErrorKind::PermissionDenied);
        let all = gather(&FakeEnv::new(), &failing, &v4_only(), Family::Any, public);
        assert_eq!(all.exit_code(), 3);
    }

    #[test]
    fn a_complete_report_has_no_notes() {
        let all = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7"));
        assert_eq!(all.notes(), Vec::<String>::new());
        assert_eq!(all.exit_code(), 0);
    }

    #[test]
    fn the_family_filter_reaches_the_local_section_only() {
        let all = gather(
            &pod(),
            &lan(),
            &dual_stack(),
            Family::V4,
            answered("203.0.113.7"),
        );
        let json = all.to_json();
        assert_eq!(keys(at(&json, &["local", "egress"])), ["ipv4"]);
        assert_eq!(at(&json, &["where", "environment"]), &s("kubernetes"));
    }

    #[test]
    fn unavailable_public_json_has_a_null_ip_and_the_reason() {
        let section = PublicSection::unavailable("timed out");
        assert_eq!(section.ip, None);
        assert_eq!(section.error.as_deref(), Some("timed out"));
        assert!(!section.disagree);
        assert_eq!(at(&section.json, &["ip"]), &Json::Null);
        assert_eq!(at(&section.json, &["error"]), &s("timed out"));
    }

    #[test]
    fn the_text_has_one_heading_per_section_in_order() {
        let text = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).render_text();
        let at_heading = |heading: &str| {
            text.find(heading)
                .unwrap_or_else(|| panic!("no {heading} in\n{text}"))
        };
        let order = [
            at_heading("[public]"),
            at_heading("[local]"),
            at_heading("[where]"),
            at_heading("[highlights]"),
        ];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
        assert!(text.contains("203.0.113.7"), "{text}");
        assert!(text.contains("192.168.1.23"), "{text}");
    }

    #[test]
    fn the_text_explains_nat_with_both_addresses() {
        let text = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).render_text();
        let highlights = &text[text.find("[highlights]").unwrap()..];
        assert!(highlights.contains("NAT"), "{highlights}");
        assert!(!highlights.contains("no NAT"), "{highlights}");
        assert!(
            highlights.contains("192.168.1.23 (private)"),
            "{highlights}"
        );
        assert!(highlights.contains("203.0.113.7"), "{highlights}");
    }

    #[test]
    fn the_text_says_when_there_is_no_nat() {
        let text = overview(&FakeEnv::new(), &v4_only(), answered("192.168.1.23")).render_text();
        let highlights = &text[text.find("[highlights]").unwrap()..];
        assert!(highlights.contains("no NAT"), "{highlights}");
    }

    #[test]
    fn the_text_says_when_nat_cannot_be_judged() {
        let public = PublicSection::unavailable("offline");
        let text = overview(&FakeEnv::new(), &v4_only(), public).render_text();
        let highlights = &text[text.find("[highlights]").unwrap()..];
        assert!(highlights.contains("NAT: unknown"), "{highlights}");
    }

    #[test]
    fn the_text_for_a_pod_names_pod_node_and_public_and_marks_the_unknown() {
        let probe = FakeProbe::new().route(PROBE_V4, "10.244.1.7");
        let text = overview(&pod(), &probe, answered("203.0.113.7")).render_text();
        let highlights = &text[text.find("[highlights]").unwrap()..];
        assert!(highlights.contains("pod 10.244.1.7"), "{highlights}");
        assert!(highlights.contains("node 192.168.49.2"), "{highlights}");
        assert!(highlights.contains("egress 10.244.1.7"), "{highlights}");
        assert!(highlights.contains("public 203.0.113.7"), "{highlights}");

        let bare = FakeEnv::new().var("KUBERNETES_SERVICE_HOST", "10.96.0.1");
        let text = overview(&bare, &probe, PublicSection::unavailable("x")).render_text();
        let highlights = &text[text.find("[highlights]").unwrap()..];
        assert!(highlights.contains("pod unknown"), "{highlights}");
        assert!(highlights.contains("node unknown"), "{highlights}");
        assert!(highlights.contains("public unknown"), "{highlights}");
    }

    #[test]
    fn the_text_has_no_kubernetes_line_outside_a_pod() {
        let text = overview(&FakeEnv::new(), &v4_only(), answered("203.0.113.7")).render_text();
        assert!(!text.contains("kubernetes  "), "{text}");
    }
}
