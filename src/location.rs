//! `myip where`: does this process run on a host, in a container or in a Kubernetes pod,
//! and what identity does it carry?
//!
//! Everything comes from [`Env`]: a handful of environment variables and read-only files.
//! There is no API access and the service-account token is never opened. Each fact says
//! whether it was *declared* (an orchestrator handed us a variable or a file) or
//! *observed* (the cgroup, a mount or a marker file we looked at ourselves). Anything
//! that cannot be established is `None`, never a guess.

use std::net::IpAddr;

use crate::Context;
use crate::cli::Options;
use crate::env::{Env, RecordingEnv};
use crate::fact::{Fact, SCHEMA_VERSION, Value};
use crate::json::Json;
use crate::output;

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
    /// `/proc/self/cgroup` places the process under a `kubepods` cgroup: something the
    /// kubelet did, observed here, independent of any variable or mount.
    pub kubepods: bool,
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

const CGROUP: &str = "/proc/self/cgroup";
const MOUNTINFO: &str = "/proc/self/mountinfo";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const NAMESPACE_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";
const DOCKERENV: &str = "/.dockerenv";
const CONTAINERENV: &str = "/run/.containerenv";
const SYSTEMD_CONTAINER: &str = "/run/systemd/container";

/// Variables a Pod spec can fill from the Downward API. None of these names is a Kubernetes
/// standard: they are the convention the README's Pod snippet uses.
const DOWNWARD_VARS: &[&str] = &[
    "POD_NAME",
    "POD_NAMESPACE",
    "POD_IP",
    "POD_IPS",
    "NODE_NAME",
    "HOST_IP",
];

pub fn detect(env: &dyn Env) -> Location {
    let container = detect_container(env);
    let kubernetes = detect_kubernetes(env, container.kubepods);
    let environment = if kubernetes.detected {
        Environment::Kubernetes
    } else if container.detected {
        Environment::Container
    } else {
        Environment::Host
    };
    Location {
        environment,
        container,
        kubernetes,
    }
}

/// Prints where this process runs. Always exits 0: what cannot be established is reported
/// as unknown, never as a failure.
pub fn run(options: &Options, ctx: &mut Context<'_>) -> u8 {
    let env = RecordingEnv::new(ctx.host.env);
    let location = detect(&env);
    output::trace(options, &env, &[], ctx.err);
    let written = output::emit(
        options,
        &location.to_json(),
        Some(&location.render_text()),
        ctx.out,
    );
    output::exit_code(written, 0)
}

struct Signal {
    source: String,
    runtime: Option<String>,
    id: Option<String>,
    evidence: String,
}

fn detect_container(env: &dyn Env) -> Container {
    let (cgroup, kubepods) = match cgroup_signal(env) {
        Some((signal, kubepods)) => (Some(signal), kubepods),
        None => (None, false),
    };
    let signals: Vec<Signal> = [
        cgroup,
        systemd_signal(env),
        mountinfo_signal(env),
        containerenv_signal(env),
        dockerenv_signal(env),
    ]
    .into_iter()
    .flatten()
    .collect();

    let pick = |value: fn(&Signal) -> Option<&String>| {
        signals.iter().find_map(|signal| {
            value(signal)
                .map(|text| Fact::observed(Value::Text(text.clone()), signal.source.clone()))
        })
    };
    Container {
        detected: !signals.is_empty(),
        kubepods,
        runtime: pick(|s| s.runtime.as_ref()),
        id: pick(|s| s.id.as_ref()),
        evidence: signals.iter().map(|s| s.evidence.clone()).collect(),
    }
}

#[derive(Default)]
struct CgroupHit {
    runtime: Option<&'static str>,
    id: Option<String>,
    kubepods: bool,
}

/// The container signal from the cgroup file, and whether any line of it is under a
/// `kubepods` cgroup (the first matching line supplies the runtime and id, but every line
/// counts for kubepods).
fn cgroup_signal(env: &dyn Env) -> Option<(Signal, bool)> {
    let content = env.read_to_string(CGROUP).ok()?;
    let hits: Vec<CgroupHit> = content
        .lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .map(cgroup_hit)
        .collect();
    let kubepods = hits.iter().any(|hit| hit.kubepods);
    let hit = hits
        .into_iter()
        .find(|hit| hit.runtime.is_some() || hit.id.is_some() || hit.kubepods)?;
    let evidence = match (&hit.runtime, &hit.id) {
        (Some(runtime), _) => {
            format!("{CGROUP} places this process in a {runtime} container cgroup")
        }
        (None, Some(_)) => {
            format!("{CGROUP} places this process in a container cgroup under kubepods")
        }
        (None, None) => format!("{CGROUP} places this process under the kubepods cgroup"),
    };
    let signal = Signal {
        source: format!("file:{CGROUP}"),
        runtime: hit.runtime.map(str::to_owned),
        id: hit.id,
        evidence,
    };
    Some((signal, kubepods))
}

/// Scans one cgroup path; the deepest (last) match wins so nested containers report the
/// innermost one.
fn cgroup_hit(path: &str) -> CgroupHit {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut hit = CgroupHit::default();
    for (i, segment) in segments.iter().enumerate() {
        hit.kubepods |= segment.starts_with("kubepods");
        let scoped = [
            ("docker-", "docker"),
            ("cri-containerd-", "containerd"),
            ("crio-", "cri-o"),
            ("libpod-", "podman"),
        ]
        .into_iter()
        .find_map(|(prefix, runtime)| scope_id(segment, prefix).map(|id| (runtime, id)));
        let next = segments.get(i + 1).copied();
        if let Some((runtime, id)) = scoped {
            hit.runtime = Some(runtime);
            hit.id = Some(id);
        } else if *segment == "docker" && next.is_some_and(is_container_id) {
            hit.runtime = Some("docker");
            hit.id = next.map(str::to_owned);
        } else if *segment == "lxc" && next.is_some_and(valid_name) {
            hit.runtime = Some("lxc");
            hit.id = next.map(str::to_owned);
        } else if let Some(name) = segment
            .strip_prefix("lxc.payload.")
            .filter(|n| valid_name(n))
        {
            hit.runtime = Some("lxc");
            hit.id = Some(name.to_owned());
        }
    }
    if hit.id.is_none() && hit.kubepods {
        hit.id = segments
            .last()
            .filter(|last| is_container_id(last))
            .map(|last| (*last).to_owned());
    }
    hit
}

fn scope_id(segment: &str, prefix: &str) -> Option<String> {
    let id = segment.strip_prefix(prefix)?.strip_suffix(".scope")?;
    is_container_id(id).then(|| id.to_owned())
}

fn is_container_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn systemd_signal(env: &dyn Env) -> Option<Signal> {
    if !env.exists(SYSTEMD_CONTAINER) {
        return None;
    }
    let content = env.read_to_string(SYSTEMD_CONTAINER).unwrap_or_default();
    let name = content.trim();
    let valid = !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'));
    Some(Signal {
        source: format!("file:{SYSTEMD_CONTAINER}"),
        runtime: valid.then(|| name.to_owned()),
        id: None,
        evidence: if valid {
            format!("{SYSTEMD_CONTAINER} names the container manager {name}")
        } else {
            format!("{SYSTEMD_CONTAINER} exists but names no recognizable container manager")
        },
    })
}

/// Under a private cgroup namespace `/proc/self/cgroup` is just `0::/`, but the runtime's
/// bind mounts of `hostname`, `hosts` and `resolv.conf` still carry the container id in
/// their source path (field 4 of each mountinfo line).
fn mountinfo_signal(env: &dyn Env) -> Option<Signal> {
    let content = env.read_to_string(MOUNTINFO).ok()?;
    let (runtime, id) = content
        .lines()
        .filter_map(|line| line.split(' ').nth(3))
        .find_map(container_mount)?;
    Some(Signal {
        source: format!("file:{MOUNTINFO}"),
        runtime: Some(runtime.to_owned()),
        id: Some(id),
        evidence: format!("{MOUNTINFO} shows {runtime} bind-mounting this container's files"),
    })
}

fn container_mount(root: &str) -> Option<(&'static str, String)> {
    const FILES: [&str; 3] = ["hostname", "hosts", "resolv.conf"];
    let after = |marker: &str, tail: &str| {
        root.match_indices(marker).find_map(|(at, _)| {
            let rest = &root[at + marker.len()..];
            let id = rest.get(..64).filter(|id| is_container_id(id))?;
            let file = rest[64..].strip_prefix(tail)?;
            FILES.contains(&file).then(|| id.to_owned())
        })
    };
    after("/containers/", "/")
        .map(|id| ("docker", id))
        .or_else(|| after("/overlay-containers/", "/userdata/").map(|id| ("podman", id)))
}

fn containerenv_signal(env: &dyn Env) -> Option<Signal> {
    if !env.exists(CONTAINERENV) {
        return None;
    }
    let id = env
        .read_to_string(CONTAINERENV)
        .ok()
        .and_then(|content| containerenv_id(&content));
    Some(Signal {
        source: format!("file:{CONTAINERENV}"),
        runtime: Some("podman".to_owned()),
        id,
        evidence: format!("{CONTAINERENV} exists (Podman creates it in its containers)"),
    })
}

fn containerenv_id(content: &str) -> Option<String> {
    let value = content
        .lines()
        .find_map(|line| line.strip_prefix("id=\""))?
        .strip_suffix('"')?;
    (12..=64)
        .contains(&value.len())
        .then_some(value)
        .filter(|v| v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .map(str::to_owned)
}

fn dockerenv_signal(env: &dyn Env) -> Option<Signal> {
    env.exists(DOCKERENV).then(|| Signal {
        source: format!("file:{DOCKERENV}"),
        runtime: Some("docker".to_owned()),
        id: None,
        evidence: format!("{DOCKERENV} exists (Docker creates it in its containers)"),
    })
}

struct ClusterDns {
    namespace: String,
    domain: String,
    nameservers: Vec<String>,
}

/// A kubelet writes `search <ns>.svc.<domain> svc.<domain> <domain>`. Both the namespaced
/// and the bare `svc.` entry must be present, so a single corporate `*.svc.example.com`
/// search domain is not mistaken for a cluster.
fn parse_resolv(content: &str) -> Option<ClusterDns> {
    let mut search: Vec<&str> = Vec::new();
    let mut nameservers: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.split(['#', ';']).next().unwrap_or_default();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("search") => search = words.collect(),
            Some("nameserver") => {
                if let Some(ip) = words.next().and_then(|w| w.parse::<IpAddr>().ok()) {
                    nameservers.push(ip.to_string());
                }
            }
            _ => {}
        }
    }
    let (namespace, domain) = search.iter().find_map(|entry| {
        let (namespace, domain) = entry.split_once(".svc.")?;
        let bare = format!("svc.{domain}");
        (valid_label(namespace) && valid_domain(domain) && search.contains(&bare.as_str()))
            .then(|| (namespace.to_owned(), domain.to_owned()))
    })?;
    Some(ClusterDns {
        namespace,
        domain,
        nameservers,
    })
}

fn valid_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 253
        && !text.contains("..")
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// A Kubernetes namespace name: a lowercase RFC 1123 label (at most 63 characters of
/// `a-z`, `0-9` and `-`, starting and ending with an alphanumeric). Also the only
/// charset that reaches output from a resolver file or a namespace file.
fn valid_label(text: &str) -> bool {
    let edge = |b: Option<&u8>| b.is_some_and(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9'));
    text.len() <= 63
        && edge(text.as_bytes().first())
        && edge(text.as_bytes().last())
        && text
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// A cluster domain such as `cluster.local`: dot-separated [`valid_label`]s, at most 253
/// characters.
fn valid_domain(text: &str) -> bool {
    text.len() <= 253 && text.split('.').all(valid_label)
}

fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn parse_ip(text: &str) -> Option<Value> {
    text.parse::<IpAddr>()
        .ok()
        .map(|ip| Value::Text(ip.to_string()))
}

fn parse_ip_list(text: &str) -> Option<Value> {
    let ips: Option<Vec<String>> = text
        .split(',')
        .map(|part| part.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string()))
        .collect();
    ips.map(Value::List)
}

fn parse_name(text: &str) -> Option<Value> {
    valid_name(text).then(|| Value::Text(text.to_owned()))
}

fn parse_label(text: &str) -> Option<Value> {
    valid_label(text).then(|| Value::Text(text.to_owned()))
}

fn parse_port(text: &str) -> Option<Value> {
    text.parse::<u16>().ok().map(|p| Value::Text(p.to_string()))
}

fn parse_host(text: &str) -> Option<Value> {
    if text.parse::<IpAddr>().is_ok() || valid_name(text) {
        Some(Value::Text(text.to_owned()))
    } else {
        None
    }
}

/// A declared value from the environment. A non-empty value that fails validation is
/// dropped (never echoed) and noted in `evidence`.
fn declared_env(
    env: &dyn Env,
    evidence: &mut Vec<String>,
    var: &str,
    parse: fn(&str) -> Option<Value>,
    expected: &str,
) -> Option<Fact> {
    let raw = clean(env.var(var))?;
    match parse(&raw) {
        Some(value) => Some(Fact::declared(value, format!("env:{var}"))),
        None => {
            evidence.push(format!("env {var} ignored: not {expected}"));
            None
        }
    }
}

fn detect_kubernetes(env: &dyn Env, kubepods: bool) -> Kubernetes {
    let mut k = Kubernetes::default();
    let service_host = clean(env.var("KUBERNETES_SERVICE_HOST"));
    let namespace_file_present = env.exists(NAMESPACE_FILE);
    let namespace_file = if namespace_file_present {
        env.read_to_string(NAMESPACE_FILE).ok()
    } else {
        None
    };
    let resolv = env
        .read_to_string(RESOLV_CONF)
        .ok()
        .and_then(|content| parse_resolv(&content));

    if service_host.is_some() {
        k.evidence
            .push("env KUBERNETES_SERVICE_HOST is set".to_owned());
    }
    if namespace_file_present {
        k.evidence.push(format!("{NAMESPACE_FILE} exists"));
    }
    if let Some(dns) = &resolv {
        k.evidence.push(format!(
            "{RESOLV_CONF} searches {}.svc.{} and svc.{}",
            dns.namespace, dns.domain, dns.domain
        ));
    }
    if kubepods {
        k.evidence.push(format!(
            "observed: {CGROUP} places this process under the kubepods cgroup"
        ));
    }
    k.detected = !k.evidence.is_empty();
    if !k.detected {
        let set: Vec<&str> = DOWNWARD_VARS
            .iter()
            .copied()
            .filter(|name| clean(env.var(name)).is_some())
            .collect();
        if !set.is_empty() {
            k.evidence.push(format!(
                "{} set, but no other Kubernetes signal was found; ignored",
                set.join(", ")
            ));
        }
        return k;
    }

    k.service_host = declared_env(
        env,
        &mut k.evidence,
        "KUBERNETES_SERVICE_HOST",
        parse_host,
        "an address or host name",
    );
    k.service_port = declared_env(
        env,
        &mut k.evidence,
        "KUBERNETES_SERVICE_PORT",
        parse_port,
        "a port number",
    );
    k.pod_name = declared_env(env, &mut k.evidence, "POD_NAME", parse_name, "a valid name");
    k.pod_ip = declared_env(env, &mut k.evidence, "POD_IP", parse_ip, "an IP address");
    k.pod_ips = declared_env(
        env,
        &mut k.evidence,
        "POD_IPS",
        parse_ip_list,
        "a comma-separated list of IP addresses",
    );
    k.node_name = declared_env(
        env,
        &mut k.evidence,
        "NODE_NAME",
        parse_name,
        "a valid name",
    );
    k.host_ip = declared_env(env, &mut k.evidence, "HOST_IP", parse_ip, "an IP address");

    k.namespace = declared_env(
        env,
        &mut k.evidence,
        "POD_NAMESPACE",
        parse_label,
        "a valid namespace name",
    );
    if k.namespace.is_none() {
        let from_file = namespace_file
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        match from_file {
            Some(text) if valid_label(text) => {
                k.namespace = Some(Fact::declared(
                    Value::Text(text.to_owned()),
                    format!("file:{NAMESPACE_FILE}"),
                ));
            }
            Some(_) => k.evidence.push(format!(
                "{NAMESPACE_FILE} ignored: not a valid namespace name"
            )),
            None => {}
        }
    }
    if let Some(dns) = &resolv {
        let source = format!("file:{RESOLV_CONF}");
        if k.namespace.is_none() {
            k.namespace = Some(Fact::declared(
                Value::Text(dns.namespace.clone()),
                source.clone(),
            ));
        }
        k.cluster_domain = Some(Fact::declared(
            Value::Text(dns.domain.clone()),
            source.clone(),
        ));
        if !dns.nameservers.is_empty() {
            k.cluster_dns = Some(Fact::declared(Value::List(dns.nameservers.clone()), source));
        }
    }
    k.host_network = host_network(&k);
    k
}

/// True when the node address is one of the pod's own addresses, which is what
/// `hostNetwork: true` looks like. Derived purely from declared values.
fn host_network(k: &Kubernetes) -> Option<Fact> {
    let host: IpAddr = k.host_ip.as_ref()?.text().parse().ok()?;
    let mut pod: Vec<IpAddr> = Vec::new();
    let mut sources = vec!["env:HOST_IP"];
    if let Some(fact) = &k.pod_ip {
        pod.extend(fact.text().parse::<IpAddr>().ok());
        sources.push("env:POD_IP");
    }
    if let Some(Fact {
        value: Value::List(items),
        ..
    }) = &k.pod_ips
    {
        pod.extend(items.iter().filter_map(|item| item.parse::<IpAddr>().ok()));
        sources.push("env:POD_IPS");
    }
    if pod.is_empty() {
        return None;
    }
    Some(Fact::declared(
        Value::Flag(pod.contains(&host)),
        sources.join(","),
    ))
}

impl Location {
    pub fn to_json(&self) -> Json {
        let c = &self.container;
        let k = &self.kubernetes;
        Json::object([
            ("schema_version", SCHEMA_VERSION.into()),
            ("environment", self.environment.as_str().into()),
            (
                "container",
                Json::object([
                    ("detected", c.detected.into()),
                    ("runtime", Json::from(&c.runtime)),
                    ("id", Json::from(&c.id)),
                    ("evidence", c.evidence.clone().into()),
                ]),
            ),
            (
                "kubernetes",
                Json::object([
                    ("detected", k.detected.into()),
                    ("service_host", Json::from(&k.service_host)),
                    ("service_port", Json::from(&k.service_port)),
                    ("namespace", Json::from(&k.namespace)),
                    ("pod_name", Json::from(&k.pod_name)),
                    ("pod_ip", Json::from(&k.pod_ip)),
                    ("pod_ips", Json::from(&k.pod_ips)),
                    ("node_name", Json::from(&k.node_name)),
                    ("host_ip", Json::from(&k.host_ip)),
                    ("host_network", Json::from(&k.host_network)),
                    ("cluster_dns", Json::from(&k.cluster_dns)),
                    ("cluster_domain", Json::from(&k.cluster_domain)),
                    ("evidence", k.evidence.clone().into()),
                ]),
            ),
        ])
    }

    /// Human-readable report. Not a stable format; `--json` is the contract.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let headline = match self.environment {
            Environment::Host => "host (no container or Kubernetes signals found)",
            Environment::Container => "container",
            Environment::Kubernetes => "kubernetes (a pod)",
        };
        row(&mut out, "environment", headline.to_owned());
        let c = &self.container;
        if c.detected {
            row(&mut out, "runtime", fact_or(&c.runtime, "unknown", whole));
            row(
                &mut out,
                "container id",
                fact_or(&c.id, "unknown", short_id),
            );
        }
        let k = &self.kubernetes;
        if k.detected {
            let downward = |var: &str| format!("unknown (expose {var} with the Downward API)");
            let rows = [
                ("namespace", &k.namespace, downward("POD_NAMESPACE")),
                ("pod name", &k.pod_name, downward("POD_NAME")),
                ("pod ip", &k.pod_ip, downward("POD_IP")),
                ("pod ips", &k.pod_ips, downward("POD_IPS")),
                ("node name", &k.node_name, downward("NODE_NAME")),
                ("host ip", &k.host_ip, downward("HOST_IP")),
                (
                    "host network",
                    &k.host_network,
                    "unknown (needs both HOST_IP and POD_IP)".to_owned(),
                ),
                ("service host", &k.service_host, "unknown".to_owned()),
                ("service port", &k.service_port, "unknown".to_owned()),
                (
                    "cluster dns",
                    &k.cluster_dns,
                    "unknown (resolv.conf has no cluster search path)".to_owned(),
                ),
                (
                    "cluster domain",
                    &k.cluster_domain,
                    "unknown (resolv.conf has no cluster search path)".to_owned(),
                ),
            ];
            for (label, fact, missing) in rows {
                row(&mut out, label, fact_or(fact, &missing, whole));
            }
        }
        let evidence: Vec<&String> = c.evidence.iter().chain(&k.evidence).collect();
        if !evidence.is_empty() {
            out.push_str("evidence\n");
            for line in evidence {
                out.push_str(&format!("  - {line}\n"));
            }
        }
        out.push_str(
            "\"declared\" values were handed to this process (environment, files an orchestrator wrote); \"observed\" ones were measured here.\n",
        );
        out
    }
}

fn row(out: &mut String, label: &str, value: String) {
    out.push_str(&format!("{label:<15}{value}\n"));
}

fn short_id(text: &str) -> String {
    let short: String = text.chars().take(12).collect();
    if short.len() < text.len() {
        format!("{short}...")
    } else {
        short
    }
}

fn whole(text: &str) -> String {
    text.to_owned()
}

fn fact_or(fact: &Option<Fact>, missing: &str, shorten: fn(&str) -> String) -> String {
    match fact {
        Some(fact) => format!(
            "{}  ({}: {})",
            shorten(&fact.text()),
            fact.basis.as_str(),
            fact.source
        ),
        None => missing.to_owned(),
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

    fn kubepods_cgroup() -> String {
        format!(
            "0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod1c2f.slice/cri-containerd-{}.scope\n",
            id64()
        )
    }

    /// A pod with `dnsPolicy: Default`, no service links and no ServiceAccount mount: the
    /// only Kubernetes evidence is the cgroup path.
    fn default_dns_pod() -> FakeEnv {
        FakeEnv::new()
            .file(CGROUP, &kubepods_cgroup())
            .file(RESOLV, "search example.com\nnameserver 192.168.1.1\n")
            .var("POD_NAME", "web-7d9f")
            .var("POD_NAMESPACE", "prod")
            .var("POD_IP", "10.1.2.3")
            .var("NODE_NAME", "node-a")
    }

    #[test]
    fn a_kubepods_cgroup_makes_this_a_pod_even_without_service_env_or_mount() {
        let location = detect(&default_dns_pod());
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
        assert!(location.container.detected);
        assert_eq!(
            text(&location.container.runtime).as_deref(),
            Some("containerd")
        );
        let k = &location.kubernetes;
        assert_eq!(text(&k.pod_name).as_deref(), Some("web-7d9f"));
        assert_eq!(source(&k.pod_name), Some("env:POD_NAME"));
        assert_eq!(basis(&k.pod_name), Some(Basis::Declared));
        assert_eq!(text(&k.pod_ip).as_deref(), Some("10.1.2.3"));
        assert_eq!(text(&k.node_name).as_deref(), Some("node-a"));
        assert_eq!(text(&k.namespace).as_deref(), Some("prod"));
        assert!(
            !k.evidence.iter().any(|line| line.contains("ignored")),
            "{:?}",
            k.evidence
        );
    }

    #[test]
    fn the_kubepods_evidence_says_observed_and_names_its_source() {
        let k = detect(&default_dns_pod()).kubernetes;
        let line = k
            .evidence
            .iter()
            .find(|line| line.contains("kubepods"))
            .unwrap_or_else(|| panic!("no kubepods evidence in {:?}", k.evidence));
        assert!(line.contains("observed"), "{line}");
        assert!(line.contains(CGROUP), "{line}");
    }

    #[test]
    fn a_kubepods_cgroup_alone_is_a_pod_with_nothing_declared() {
        let env = FakeEnv::new().file(CGROUP, &kubepods_cgroup());
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
        assert!(location.kubernetes.pod_name.is_none());
        assert!(location.kubernetes.namespace.is_none());
    }

    #[test]
    fn a_kubepods_cgroup_without_a_container_id_is_still_a_pod() {
        let env = FakeEnv::new().file(CGROUP, "0::/kubepods/besteffort/pod1c2f\n");
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
    }

    #[test]
    fn kubepods_on_a_later_cgroup_line_still_makes_this_a_pod() {
        let cgroup = format!(
            "12:pids:/\n4:cpu,cpuacct:/\n1:name=systemd:/kubepods/besteffort/pod1c2f/{}\n0::/\n",
            id64()
        );
        let location = detect(&FakeEnv::new().file(CGROUP, &cgroup));
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
        assert!(location.container.detected);
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn kubepods_on_the_first_cgroup_line_only_still_makes_this_a_pod() {
        let cgroup = format!(
            "12:pids:/kubepods/besteffort/pod1c2f/{}\n4:cpu,cpuacct:/\n1:name=systemd:/\n0::/\n",
            id64()
        );
        let location = detect(&FakeEnv::new().file(CGROUP, &cgroup));
        assert_eq!(location.environment, Environment::Kubernetes);
        assert!(location.kubernetes.detected);
        assert_eq!(text(&location.container.id), Some(id64()));
    }

    #[test]
    fn a_cgroup_without_kubepods_on_any_line_is_not_a_pod() {
        let cgroup = format!(
            "12:pids:/docker/{id}\n4:cpu,cpuacct:/docker/{id}\n1:name=systemd:/docker/{id}\n0::/\n",
            id = id64()
        );
        let location = detect(&FakeEnv::new().file(CGROUP, &cgroup));
        assert_eq!(location.environment, Environment::Container);
        assert!(!location.kubernetes.detected);
    }

    #[test]
    fn a_runtime_line_before_a_kubepods_line_keeps_its_runtime_and_the_pod() {
        let cgroup = format!(
            "2:cpu:/docker/{}\n1:name=systemd:/kubepods/besteffort/pod1c2f\n",
            id64()
        );
        let location = detect(&FakeEnv::new().file(CGROUP, &cgroup));
        assert_eq!(location.environment, Environment::Kubernetes);
        assert_eq!(text(&location.container.runtime).as_deref(), Some("docker"));
    }

    #[test]
    fn a_plain_container_cgroup_does_not_make_pod_variables_count() {
        let env = FakeEnv::new()
            .file(CGROUP, &cgroup_v1(&format!("/docker/{}", id64())))
            .var("POD_NAME", "web");
        let location = detect(&env);
        assert_eq!(location.environment, Environment::Container);
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
    fn the_cluster_domain_is_declared_by_the_resolv_conf() {
        let location = detect(&FakeEnv::new().file(RESOLV, KUBE_RESOLV));
        let domain = location.kubernetes.cluster_domain.as_ref().unwrap();
        assert_eq!(domain.basis, Basis::Declared);
        assert_eq!(domain.source, "file:/etc/resolv.conf");
    }

    #[test]
    fn a_search_domain_that_is_not_a_plain_dns_name_is_not_a_cluster_signal() {
        let hostile = [
            "\u{1b}[2J",
            "\u{1b}[31mx",
            "a\u{0}b",
            "a\u{202e}b",
            "a..b",
            "a_b",
            "-x",
            "x-",
            "UPPER.case",
        ];
        for bad in hostile {
            let resolv = format!("search prod.svc.{bad} svc.{bad}\nnameserver 10.96.0.10\n");
            let alone = detect(&FakeEnv::new().file(RESOLV, &resolv));
            assert!(!alone.kubernetes.detected, "{bad:?} was accepted");
            let with_service = detect(
                &FakeEnv::new()
                    .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
                    .file(RESOLV, &resolv),
            );
            let k = with_service.kubernetes;
            assert!(k.cluster_domain.is_none(), "{bad:?}");
            assert!(k.cluster_dns.is_none(), "{bad:?}");
            assert!(k.namespace.is_none(), "{bad:?}");
            let everything = format!("{:?}", k.evidence);
            assert!(!everything.contains(bad), "{bad:?} reached {everything}");
        }
    }

    #[test]
    fn an_over_long_search_domain_is_not_a_cluster_signal() {
        let label = "a".repeat(63);
        let domain = [label.as_str(); 4].join(".");
        assert!(domain.len() > 253);
        let resolv = format!("search prod.svc.{domain} svc.{domain}\n");
        assert!(
            !detect(&FakeEnv::new().file(RESOLV, &resolv))
                .kubernetes
                .detected
        );
    }

    #[test]
    fn a_label_is_a_lowercase_dns_1123_label() {
        let longest = "a".repeat(63);
        for good in ["prod", "kube-system", "a", "0abc", "a-1", longest.as_str()] {
            assert!(valid_label(good), "{good:?} should be valid");
        }
        let too_long = "a".repeat(64);
        for bad in [
            "",
            "Prod",
            "PROD",
            "pr.od",
            "pr od",
            "pr_od",
            "-prod",
            "prod-",
            "pr\u{1b}[2Jod",
            "pr\nod",
            "pr\0od",
            "prodé",
            "a\u{1b}cb",
            "prOd",
            "pr\u{7f}od",
            "pr\u{85}od",
            "pr\u{202e}od",
            too_long.as_str(),
        ] {
            assert!(!valid_label(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn a_domain_is_dot_separated_valid_labels() {
        let long = ["a"; 127].join(".");
        assert_eq!(long.len(), 253);
        for good in ["cluster.local", "a.b.c", "corp-1.example", long.as_str()] {
            assert!(valid_domain(good), "{good:?} should be valid");
        }
        let too_long = format!("{long}a");
        for bad in [
            "",
            ".",
            "a..b",
            "cluster.local.",
            ".cluster.local",
            "Cluster.local",
            "clu\u{1b}ster.local",
            "cluster.lo\u{1b}[2Jcal",
            "cluster.local\n",
            too_long.as_str(),
        ] {
            assert!(!valid_domain(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn a_label_is_judged_on_its_interior_characters_too() {
        for interior in ["a\u{1b}cb", "prOd", "a.b", "a_b", "a b", "a\u{e9}b"] {
            let edges_ok = interior.starts_with(|c: char| c.is_ascii_lowercase())
                && interior.ends_with(|c: char| c.is_ascii_lowercase());
            assert!(edges_ok, "{interior:?} must differ only in the interior");
            assert!(!valid_label(interior), "{interior:?} should be invalid");
        }
        assert!(valid_label("a-b"));
    }

    #[test]
    fn every_namespace_source_requires_a_valid_label() {
        let bad = [
            "Prod",
            "pr.od",
            "pr\u{1b}[2Jod",
            "pr_od",
            "-prod",
            "a\u{1b}cb",
            "prOd",
        ];
        for ns in bad {
            let from_env = detect(
                &FakeEnv::new()
                    .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
                    .var("POD_NAMESPACE", ns),
            );
            assert!(from_env.kubernetes.namespace.is_none(), "env {ns:?}");
            let from_file = detect(&FakeEnv::new().file(NAMESPACE_FILE, &format!("{ns}\n")));
            assert!(from_file.kubernetes.detected);
            assert!(from_file.kubernetes.namespace.is_none(), "file {ns:?}");
            let resolv = format!("search {ns}.svc.cluster.local svc.cluster.local\n");
            let from_dns = detect(&FakeEnv::new().file(RESOLV, &resolv));
            assert!(!from_dns.kubernetes.detected, "resolv {ns:?}");
        }
        let env = FakeEnv::new()
            .var("KUBERNETES_SERVICE_HOST", "10.96.0.1")
            .var("POD_NAMESPACE", &"a".repeat(64));
        assert!(detect(&env).kubernetes.namespace.is_none());
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
