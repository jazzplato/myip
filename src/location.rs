//! `myip where`: does this process run on a host, in a container or in a Kubernetes pod,
//! and what identity does it carry?
//!
//! Everything comes from [`Env`]: a handful of environment variables and read-only files.
//! There is no API access and the service-account token is never opened. Each fact says
//! whether it was *declared* (an orchestrator handed us a variable or a file) or
//! *observed* (the cgroup, a mount or a marker file we looked at ourselves). Anything
//! that cannot be established is `None`, never a guess.

use crate::env::Env;
use crate::fact::Fact;
use crate::json::Json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Host,
    Container,
    Kubernetes,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Container => "container",
            Self::Kubernetes => "kubernetes",
        }
    }
}

#[derive(Debug, Default)]
pub struct Container {
    pub detected: bool,
    pub runtime: Option<Fact>,
    pub id: Option<Fact>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Kubernetes {
    pub detected: bool,
    pub service_host: Option<Fact>,
    pub service_port: Option<Fact>,
    pub namespace: Option<Fact>,
    pub pod_name: Option<Fact>,
    pub pod_ip: Option<Fact>,
    pub pod_ips: Option<Fact>,
    pub node_name: Option<Fact>,
    pub host_ip: Option<Fact>,
    pub host_network: Option<Fact>,
    pub cluster_dns: Option<Fact>,
    pub cluster_domain: Option<Fact>,
    pub evidence: Vec<String>,
}

#[derive(Debug)]
pub struct Location {
    pub environment: Environment,
    pub container: Container,
    pub kubernetes: Kubernetes,
}

pub fn detect(_env: &dyn Env) -> Location {
    unimplemented!()
}

impl Location {
    pub fn to_json(&self) -> Json {
        unimplemented!()
    }

    pub fn render_text(&self) -> String {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::testing::FakeEnv;
    use crate::env::{ALLOWED_PATHS, ALLOWED_VARS, Access, RecordingEnv};
    use crate::fact::{Basis, Value};

    const CGROUP: &str = "/proc/self/cgroup";
    const MOUNTINFO: &str = "/proc/self/mountinfo";
    const RESOLV: &str = "/etc/resolv.conf";
    const NAMESPACE_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";
    const TOKEN_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
    const CA_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
    const KUBE_RESOLV: &str = "search prod.svc.cluster.local svc.cluster.local cluster.local\nnameserver 10.96.0.10\noptions ndots:5\n";

    fn id64() -> String {
        "0123456789abcdef".repeat(4)
    }

    fn text(fact: &Option<Fact>) -> Option<String> {
        fact.as_ref().map(Fact::text)
    }

    fn source(fact: &Option<Fact>) -> Option<&str> {
        fact.as_ref().map(|f| f.source.as_str())
    }

    fn basis(fact: &Option<Fact>) -> Option<Basis> {
        fact.as_ref().map(|f| f.basis)
    }

    fn cgroup_v1(path: &str) -> String {
        format!("12:pids:{path}\n4:cpu,cpuacct:{path}\n1:name=systemd:{path}\n0::{path}\n")
    }

    fn pod_env() -> FakeEnv {
        FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("KUBERNETES_SERVICE_PORT", "443")
            .var("POD_NAME", "web-7d9f")
            .var("POD_NAMESPACE", "prod")
            .var("POD_IP", "10.1.2.3")
            .var("POD_IPS", "10.1.2.3,fd00::3")
            .var("NODE_NAME", "node-a")
            .var("HOST_IP", "192.168.1.10")
            .file(NAMESPACE_FILE, "prod\n")
            .file(TOKEN_FILE, "SECRET-TOKEN-VALUE")
            .file(CA_FILE, "SECRET-CA-VALUE")
            .file(RESOLV, KUBE_RESOLV)
            .file(
                CGROUP,
                &format!(
                    "0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod1c2f.slice/cri-containerd-{}.scope\n",
                    id64()
                ),
            )
    }

    #[test]
    fn a_bare_host_is_a_host_with_nothing_declared() {
        let location = detect(&FakeEnv::new());
        assert_eq!(location.environment, Environment::Host);
        assert!(!location.container.detected);
        assert!(location.container.runtime.is_none() && location.container.id.is_none());
        assert!(!location.kubernetes.detected);
        assert!(location.kubernetes.pod_ip.is_none());
        assert!(location.container.evidence.is_empty());
    }

    #[test]
    fn a_host_renders_the_exact_json_document() {
        let json = detect(&FakeEnv::new()).to_json().render(true);
        assert_eq!(
            json,
            r#"{
  "schema_version": 1,
  "environment": "host",
  "container": {
    "detected": false,
    "runtime": null,
    "id": null,
    "evidence": []
  },
  "kubernetes": {
    "detected": false,
    "service_host": null,
    "service_port": null,
    "namespace": null,
    "pod_name": null,
    "pod_ip": null,
    "pod_ips": null,
    "node_name": null,
    "host_ip": null,
    "host_network": null,
    "cluster_dns": null,
    "cluster_domain": null,
    "evidence": []
  }
}"#
        );
    }

    #[test]
    fn docker_on_cgroup_v1_gives_runtime_and_id() {
        let env = FakeEnv::new().file(CGROUP, &cgroup_v1(&format!("/docker/{}", id64())));
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Container);
        assert!(location.container.detected);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("docker"));
        assert_eq!(text(&location.container.id), Some(id64()));
        assert_eq!(
            source(&location.container.id),
            Some("file:/proc/self/cgroup")
        );
        assert_eq!(basis(&location.container.id), Some(Basis::Observed));
        assert_eq!(basis(&location.container.runtime), Some(Basis::Observed));
    }

    #[test]
    fn docker_with_the_systemd_driver_on_cgroup_v2() {
        let env = FakeEnv::new().file(
            CGROUP,
            &format!("0::/system.slice/docker-{}.scope\n", id64()),
        );
        let location = detect(&env);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("docker"));
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn a_private_cgroup_namespace_falls_back_to_the_docker_bind_mounts() {
        let mountinfo = format!(
            "2315 2300 0:140 / / rw,relatime master:1 - overlay overlay rw\n\
             2330 2315 8:1 /var/lib/docker/containers/{id}/resolv.conf /etc/resolv.conf rw,relatime - ext4 /dev/sda1 rw\n\
             2331 2315 8:1 /var/lib/docker/containers/{id}/hostname /etc/hostname rw,relatime - ext4 /dev/sda1 rw\n",
            id = id64()
        );
        let env = FakeEnv::new()
            .file(CGROUP, "0::/\n")
            .file(MOUNTINFO, &mountinfo);
        let location = detect(&env);
        assert!(location.container.detected);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("docker"));
        assert_eq!(text(&location.container.id), Some(id64()));
        assert_eq!(
            source(&location.container.id),
            Some("file:/proc/self/mountinfo")
        );
    }

    #[test]
    fn a_relocated_docker_root_is_recognised_by_the_containers_directory() {
        let mountinfo = format!(
            "2330 2315 8:1 /containers/{id}/hostname /etc/hostname rw - ext4 /dev/sdb1 rw\n",
            id = id64()
        );
        let env = FakeEnv::new().file(MOUNTINFO, &mountinfo);
        assert_eq!(text(&detect(&env).container.id), Some(id64()));
    }

    #[test]
    fn a_docker_host_listing_a_container_shm_mount_is_still_a_host() {
        let mountinfo = format!(
            "901 88 0:55 / /var/lib/docker/containers/{}/mounts/shm rw - tmpfs shm rw\n",
            id64()
        );
        let env = FakeEnv::new()
            .file(CGROUP, "0::/user.slice/user-1000.slice/session-3.scope\n")
            .file(MOUNTINFO, &mountinfo);
        assert_eq!(detect(&env).environment, Environment::Host);
    }

    #[test]
    fn the_docker_daemon_cgroup_on_a_host_is_not_a_container() {
        let env = FakeEnv::new().file(
            CGROUP,
            "12:pids:/system.slice/docker.service\n0::/system.slice/containerd.service\n",
        );
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Host);
        assert!(location.container.runtime.is_none());
    }

    #[test]
    fn containerd_under_kubepods() {
        let env = FakeEnv::new().file(
            CGROUP,
            &format!(
                "0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod1c2f.slice/cri-containerd-{}.scope\n",
                id64()
            ),
        );
        let location = detect(&env);
        assert_eq!(
            text(&location.container.runtime).as_deref(),
            Some("containerd")
        );
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn cri_o_under_kubepods() {
        let env = FakeEnv::new().file(
            CGROUP,
            &format!(
                "0::/kubepods.slice/kubepods-besteffort.slice/kubepods-besteffort-pod9a.slice/crio-{}.scope\n",
                id64()
            ),
        );
        let location = detect(&env);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("cri-o"));
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn rootless_podman_on_cgroup_v2() {
        let env = FakeEnv::new().file(
            CGROUP,
            &format!(
                "0::/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-{}.scope/container\n",
                id64()
            ),
        );
        let location = detect(&env);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("podman"));
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn podman_marker_file_gives_runtime_and_the_container_id() {
        let containerenv = format!(
            "engine=\"podman-4.9.3\"\nname=\"web\"\nid=\"{}\"\nimage=\"docker.io/library/nginx:latest\"\n",
            id64()
        );
        let env = FakeEnv::new().file("/run/.containerenv", &containerenv);
        let location = detect(&env);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("podman"));
        assert_eq!(text(&location.container.id), Some(id64()));
        assert_eq!(
            source(&location.container.runtime),
            Some("file:/run/.containerenv")
        );
    }

    #[test]
    fn lxc_both_cgroup_layouts_use_the_container_name_as_id() {
        let old = FakeEnv::new().file(CGROUP, "0::/lxc/c1\n");
        let new = FakeEnv::new().file(CGROUP, "0::/lxc.payload.c1/init.scope\n");
        for env in [old, new] {
            let location = detect(&env);
            assert_eq!(text(&location.container.runtime).as_deref(), Some("lxc"));
            assert_eq!(text(&location.container.id).as_deref(), Some("c1"));
        }
    }

    #[test]
    fn systemd_container_file_names_the_runtime() {
        let env = FakeEnv::new().file("/run/systemd/container", "systemd-nspawn\n");
        let location = detect(&env);
        assert!(location.container.detected);
        assert_eq!(
            text(&location.container.runtime).as_deref(),
            Some("systemd-nspawn")
        );
        assert!(location.container.id.is_none());
        assert_eq!(
            source(&location.container.runtime),
            Some("file:/run/systemd/container")
        );
    }

    #[test]
    fn a_junk_systemd_container_file_is_ignored_but_still_a_signal() {
        let env = FakeEnv::new().file("/run/systemd/container", "bad value \"x\"\n");
        let location = detect(&env);
        assert!(location.container.detected);
        assert!(location.container.runtime.is_none());
    }

    #[test]
    fn dockerenv_alone_says_docker_without_an_id() {
        let env = FakeEnv::new().file("/.dockerenv", "");
        let location = detect(&env);
        assert!(location.container.detected);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("docker"));
        assert!(location.container.id.is_none());
        assert_eq!(
            source(&location.container.runtime),
            Some("file:/.dockerenv")
        );
        assert!(
            location
                .container
                .evidence
                .iter()
                .any(|line| line.contains("/.dockerenv"))
        );
    }

    #[test]
    fn kubepods_without_a_runtime_token_reports_the_container_id_only() {
        let path = format!(
            "/kubepods/burstable/pod1c2f3a4b-0000-0000-0000-000000000000/{}",
            id64()
        );
        let env = FakeEnv::new().file(CGROUP, &cgroup_v1(&path));
        let location = detect(&env);
        assert!(location.container.detected);
        assert!(location.container.runtime.is_none());
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn the_cgroup_wins_over_markers_but_every_signal_is_listed() {
        let env = FakeEnv::new()
            .file(
                CGROUP,
                &format!("0::/kubepods.slice/cri-containerd-{}.scope\n", id64()),
            )
            .file("/.dockerenv", "");
        let location = detect(&env);
        assert_eq!(
            text(&location.container.runtime).as_deref(),
            Some("containerd")
        );
        assert!(location.container.evidence.len() >= 2);
    }

    #[test]
    fn the_innermost_container_wins_when_containers_nest() {
        let inner = "fedcba9876543210".repeat(4);
        let env = FakeEnv::new().file(CGROUP, &format!("0::/docker/{}/docker/{inner}\n", id64()));
        assert_eq!(text(&detect(&env).container.id), Some(inner));
    }

    #[test]
    fn short_or_non_hex_ids_never_match() {
        let short = "a".repeat(63);
        let nonhex = "g".repeat(64);
        for bad in [short, nonhex] {
            let env = FakeEnv::new().file(CGROUP, &format!("0::/docker/{bad}\n"));
            assert_eq!(detect(&env).environment, Environment::Host, "{bad}");
        }
    }

    #[test]
    fn an_unreadable_cgroup_is_not_an_error() {
        let env = FakeEnv::new().unreadable(CGROUP).unreadable(MOUNTINFO);
        assert_eq!(detect(&env).environment, Environment::Host);
    }

    #[test]
    fn kubernetes_is_detected_from_the_service_environment_alone() {
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("KUBERNETES_SERVICE_PORT", "443");
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
        assert!(!location.container.detected);
        let host = location.kubernetes.service_host.as_ref().unwrap();
        assert_eq!(host.text(), "10.96.0.1");
        assert_eq!(host.source, "env:KUBERNETES_SERVICE_HOST");
        assert_eq!(host.basis, Basis::Declared);
        assert_eq!(
            text(&location.kubernetes.service_port).as_deref(),
            Some("443")
        );
        assert!(location.kubernetes.namespace.is_none());
        assert!(location.kubernetes.pod_ip.is_none());
    }

    #[test]
    fn the_service_account_namespace_file_is_a_signal_and_a_declared_namespace() {
        let env = FakeEnv::new().file(NAMESPACE_FILE, "prod\n");
        let location = detect(&env);
        assert!(location.kubernetes.detected);
        assert_eq!(
            text(&location.kubernetes.namespace).as_deref(),
            Some("prod")
        );
        assert_eq!(
            source(&location.kubernetes.namespace),
            Some("file:/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        );
        assert_eq!(basis(&location.kubernetes.namespace), Some(Basis::Declared));
    }

    #[test]
    fn an_unreadable_namespace_file_still_counts_as_a_signal() {
        let env = FakeEnv::new().unreadable(NAMESPACE_FILE);
        let location = detect(&env);
        assert!(location.kubernetes.detected);
        assert!(location.kubernetes.namespace.is_none());
    }

    #[test]
    fn namespace_precedence_is_env_then_file_then_resolv_conf() {
        let all = FakeEnv::new()
            .var("POD_NAMESPACE", "from-env")
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .file(NAMESPACE_FILE, "from-file\n")
            .file(
                RESOLV,
                "search from-dns.svc.cluster.local svc.cluster.local cluster.local\n",
            );
        assert_eq!(
            text(&detect(&all).kubernetes.namespace).as_deref(),
            Some("from-env")
        );
        let file_and_dns = FakeEnv::new().file(NAMESPACE_FILE, "from-file\n").file(
            RESOLV,
            "search from-dns.svc.cluster.local svc.cluster.local cluster.local\n",
        );
        assert_eq!(
            text(&detect(&file_and_dns).kubernetes.namespace).as_deref(),
            Some("from-file")
        );
        let dns = FakeEnv::new().file(
            RESOLV,
            "search from-dns.svc.cluster.local svc.cluster.local cluster.local\n",
        );
        let location = detect(&dns);
        assert_eq!(
            text(&location.kubernetes.namespace).as_deref(),
            Some("from-dns")
        );
        assert_eq!(
            source(&location.kubernetes.namespace),
            Some("file:/etc/resolv.conf")
        );
    }

    #[test]
    fn a_cluster_resolv_conf_gives_namespace_domain_and_dns() {
        let env = FakeEnv::new().file(RESOLV, KUBE_RESOLV);
        let location = detect(&env);
        assert!(location.kubernetes.detected);
        assert_eq!(
            text(&location.kubernetes.namespace).as_deref(),
            Some("prod")
        );
        assert_eq!(
            text(&location.kubernetes.cluster_domain).as_deref(),
            Some("cluster.local")
        );
        assert_eq!(
            location.kubernetes.cluster_dns.as_ref().unwrap().value,
            Value::List(vec!["10.96.0.10".to_owned()])
        );
        assert_eq!(
            basis(&location.kubernetes.cluster_dns),
            Some(Basis::Declared)
        );
    }

    #[test]
    fn a_custom_cluster_domain_is_reported_as_is() {
        let env = FakeEnv::new().file(
            RESOLV,
            "search team.svc.corp.example svc.corp.example corp.example\nnameserver 172.20.0.10\n",
        );
        let location = detect(&env);
        assert_eq!(
            text(&location.kubernetes.cluster_domain).as_deref(),
            Some("corp.example")
        );
        assert_eq!(
            text(&location.kubernetes.namespace).as_deref(),
            Some("team")
        );
    }

    #[test]
    fn an_ordinary_resolv_conf_is_not_kubernetes() {
        let env = FakeEnv::new().file(
            RESOLV,
            "# generated by NetworkManager\nsearch example.com\nnameserver 8.8.8.8\n",
        );
        let location = detect(&env);
        assert!(!location.kubernetes.detected);
        assert!(location.kubernetes.cluster_dns.is_none());
    }

    #[test]
    fn a_single_corporate_svc_search_entry_is_not_enough() {
        let env = FakeEnv::new().file(
            RESOLV,
            "search build.svc.example.com\nnameserver 10.0.0.2\n",
        );
        assert!(!detect(&env).kubernetes.detected);
    }

    #[test]
    fn resolv_conf_comments_are_ignored_and_the_last_search_line_wins() {
        let env = FakeEnv::new().file(
            RESOLV,
            "search stale.svc.old.local svc.old.local old.local\n\
             ; comment\n\
             # nameserver 1.2.3.4\n\
             search prod.svc.cluster.local svc.cluster.local cluster.local # trailing\n\
             nameserver 10.96.0.10\n\
             nameserver not-an-ip\n\
             nameserver fd00::a\n",
        );
        let location = detect(&env);
        assert_eq!(
            text(&location.kubernetes.namespace).as_deref(),
            Some("prod")
        );
        assert_eq!(
            location.kubernetes.cluster_dns.as_ref().unwrap().value,
            Value::List(vec!["10.96.0.10".to_owned(), "fd00::a".to_owned()])
        );
    }

    #[test]
    fn dnspolicy_default_leaves_cluster_dns_null_even_in_a_pod() {
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .file(RESOLV, "search example.com\nnameserver 192.168.1.1\n");
        let location = detect(&env);
        assert!(location.kubernetes.detected);
        assert!(location.kubernetes.cluster_dns.is_none());
        assert!(location.kubernetes.cluster_domain.is_none());
    }

    #[test]
    fn downward_api_variables_are_declared_facts_with_their_sources() {
        let location = detect(&pod_env());
        let k = &location.kubernetes;
        assert_eq!(text(&k.pod_name).as_deref(), Some("web-7d9f"));
        assert_eq!(source(&k.pod_name), Some("env:POD_NAME"));
        assert_eq!(text(&k.pod_ip).as_deref(), Some("10.1.2.3"));
        assert_eq!(source(&k.pod_ip), Some("env:POD_IP"));
        assert_eq!(
            k.pod_ips.as_ref().unwrap().value,
            Value::List(vec!["10.1.2.3".to_owned(), "fd00::3".to_owned()])
        );
        assert_eq!(text(&k.node_name).as_deref(), Some("node-a"));
        assert_eq!(text(&k.host_ip).as_deref(), Some("192.168.1.10"));
        assert_eq!(text(&k.namespace).as_deref(), Some("prod"));
        assert_eq!(source(&k.namespace), Some("env:POD_NAMESPACE"));
        for fact in [&k.pod_name, &k.pod_ip, &k.pod_ips, &k.node_name, &k.host_ip] {
            assert_eq!(basis(fact), Some(Basis::Declared));
        }
    }

    #[test]
    fn a_pod_on_containerd_is_kubernetes_with_the_container_block_filled_too() {
        let location = detect(&pod_env());
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.container.detected);
        assert_eq!(
            text(&location.container.runtime).as_deref(),
            Some("containerd")
        );
    }

    #[test]
    fn host_network_is_derived_from_host_ip_and_pod_ip() {
        let separate = detect(&pod_env()).kubernetes.host_network.unwrap();
        assert_eq!(separate.value, Value::Flag(false));
        assert_eq!(separate.basis, Basis::Declared);
        assert_eq!(separate.source, "env:HOST_IP,env:POD_IP,env:POD_IPS");

        let shared = pod_env().var("HOST_IP", "10.1.2.3");
        assert_eq!(
            detect(&shared).kubernetes.host_network.unwrap().value,
            Value::Flag(true)
        );
    }

    #[test]
    fn host_network_compares_addresses_not_spellings() {
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_IP", "fd00:0:0:0:0:0:0:3")
            .var("HOST_IP", "FD00::3");
        assert_eq!(
            detect(&env).kubernetes.host_network.unwrap().value,
            Value::Flag(true)
        );
    }

    #[test]
    fn host_network_is_null_when_either_side_is_missing() {
        let no_host = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_IP", "10.1.2.3");
        assert!(detect(&no_host).kubernetes.host_network.is_none());
        let no_pod = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("HOST_IP", "10.1.2.3");
        assert!(detect(&no_pod).kubernetes.host_network.is_none());
    }

    #[test]
    fn invalid_declared_values_become_null_with_a_note() {
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_IP", "not-an-ip")
            .var("POD_NAME", "evil\"name\nwith spaces")
            .var("NODE_NAME", "node/../x")
            .var("POD_IPS", "10.1.2.3,bogus");
        let k = detect(&env).kubernetes;
        assert!(k.pod_ip.is_none());
        assert!(k.pod_name.is_none());
        assert!(k.node_name.is_none());
        assert!(k.pod_ips.is_none());
        for var in ["POD_IP", "POD_NAME", "NODE_NAME", "POD_IPS"] {
            assert!(
                k.evidence.iter().any(|line| line.contains(var)),
                "no note for {var}: {:?}",
                k.evidence
            );
        }
    }

    #[test]
    fn empty_or_blank_variables_count_as_absent_without_a_note() {
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_NAME", "")
            .var("NODE_NAME", "  \n");
        let k = detect(&env).kubernetes;
        assert!(k.pod_name.is_none() && k.node_name.is_none());
        assert!(!k.evidence.iter().any(|line| line.contains("POD_NAME")));
    }

    #[test]
    fn pod_variables_on_a_machine_with_no_other_kubernetes_signal_are_ignored() {
        let env = FakeEnv::new()
            .var("POD_NAME", "web")
            .var("POD_IP", "10.1.2.3");
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Host);
        assert!(!location.kubernetes.detected);
        assert!(location.kubernetes.pod_name.is_none());
        assert!(
            location
                .kubernetes
                .evidence
                .iter()
                .any(|line| line.contains("no other Kubernetes signal")),
            "{:?}",
            location.kubernetes.evidence
        );
    }

    #[test]
    fn the_service_account_token_and_ca_are_never_touched() {
        let fake = pod_env();
        let spy = RecordingEnv::new(&fake);
        let location = detect(&spy);

        for access in spy.accesses() {
            match &access {
                Access::Read(path) | Access::Exists(path) => {
                    assert!(
                        ALLOWED_PATHS.contains(&path.as_str()),
                        "touched {path}, which is not on the allow-list"
                    );
                    assert!(
                        !path.ends_with("/token") && !path.ends_with("/ca.crt"),
                        "{access}"
                    );
                }
                Access::Var(name) => assert!(
                    ALLOWED_VARS.contains(&name.as_str()),
                    "read {name}, which is not on the allow-list"
                ),
            }
        }

        let output = format!(
            "{}\n{}\n{:?}",
            location.to_json().render(true),
            location.render_text(),
            location
        );
        assert!(!output.contains("SECRET-TOKEN-VALUE"));
        assert!(!output.contains("SECRET-CA-VALUE"));
    }

    #[test]
    fn the_namespace_file_is_the_only_serviceaccount_path_read() {
        let fake = pod_env();
        let spy = RecordingEnv::new(&fake);
        let _ = detect(&spy);
        let touched: Vec<String> = spy
            .accesses()
            .into_iter()
            .filter_map(|a| match a {
                Access::Read(p) | Access::Exists(p) => Some(p),
                Access::Var(_) => None,
            })
            .filter(|p| p.contains("serviceaccount"))
            .collect();
        assert!(!touched.is_empty());
        assert!(touched.iter().all(|p| p == NAMESPACE_FILE), "{touched:?}");
    }

    #[test]
    fn a_pod_renders_every_fact_in_json_with_its_basis() {
        let json = detect(&pod_env()).to_json().render(true);
        assert!(
            json.starts_with("{\n  \"schema_version\": 1,\n  \"environment\": \"kubernetes\",")
        );
        assert!(json.contains("\"basis\": \"declared\""));
        assert!(json.contains("\"basis\": \"observed\""));
        assert!(json.contains("\"source\": \"env:POD_IP\""));
        assert!(json.contains("\"runtime\": {\n      \"value\": \"containerd\""));
        assert!(!json.contains("SECRET"));
    }

    #[test]
    fn text_for_a_host_says_what_host_means() {
        let text = detect(&FakeEnv::new()).render_text();
        assert!(text.contains("host"), "{text}");
        assert!(
            text.contains("no container or Kubernetes signals"),
            "{text}"
        );
    }

    #[test]
    fn text_for_a_pod_labels_provenance_and_hints_at_missing_downward_api_fields() {
        let partial = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .file(NAMESPACE_FILE, "prod\n");
        let text = detect(&partial).render_text();
        assert!(text.contains("kubernetes"), "{text}");
        assert!(text.contains("prod"), "{text}");
        assert!(text.contains("declared"), "{text}");
        assert!(text.contains("unknown"), "{text}");
        assert!(text.contains("POD_IP"), "{text}");
        assert!(text.contains("Downward API"), "{text}");
    }

    #[test]
    fn text_shortens_long_container_ids_but_json_keeps_them_whole() {
        let env = FakeEnv::new().file(CGROUP, &cgroup_v1(&format!("/docker/{}", id64())));
        let location = detect(&env);
        let text = location.render_text();
        assert!(text.contains(&id64()[..12]), "{text}");
        assert!(!text.contains(&id64()), "{text}");
        assert!(location.to_json().render(true).contains(&id64()));
        assert!(text.contains("observed"), "{text}");
    }
}
