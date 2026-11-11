//! The only way `myip` reads its surroundings: environment variables and a handful of
//! read-only files. Detection code takes `&dyn Env`, so tests substitute fixtures and the
//! real implementation can refuse anything outside an allow-list.

use std::cell::RefCell;
use std::fmt;
use std::io;

/// Files `myip` may look at (existence or contents). Kubernetes credentials are
/// deliberately absent: the service-account `token` and `ca.crt` are never opened.
pub const ALLOWED_PATHS: &[&str] = &[
    "/.dockerenv",
    "/run/.containerenv",
    "/run/systemd/container",
    "/proc/self/cgroup",
    "/proc/self/mountinfo",
    "/proc/net/route",
    "/proc/net/ipv6_route",
    "/etc/resolv.conf",
    "/var/run/secrets/kubernetes.io/serviceaccount/namespace",
];

/// Environment variables `myip` may read.
pub const ALLOWED_VARS: &[&str] = &[
    "KUBERNETES_SERVICE_HOST",
    "KUBERNETES_SERVICE_PORT",
    "POD_NAME",
    "POD_NAMESPACE",
    "POD_IP",
    "POD_IPS",
    "NODE_NAME",
    "HOST_IP",
];

/// Largest file `SystemEnv` reads; every allow-listed file is far smaller.
const MAX_READ_BYTES: u64 = 1 << 20;

pub trait Env {
    fn var(&self, name: &str) -> Option<String>;
    fn read_to_string(&self, path: &str) -> io::Result<String>;
    fn exists(&self, path: &str) -> bool;
}

/// The real environment. Anything outside [`ALLOWED_PATHS`] and [`ALLOWED_VARS`] is
/// refused before the operating system is asked, by exact match, so `..` tricks and
/// symlink-looking spellings of the token path do not match anything.
pub struct SystemEnv;

impl SystemEnv {
    pub fn allows_path(path: &str) -> bool {
        ALLOWED_PATHS.contains(&path)
    }

    pub fn allows_var(name: &str) -> bool {
        ALLOWED_VARS.contains(&name)
    }
}

impl Env for SystemEnv {
    fn var(&self, _name: &str) -> Option<String> {
        unimplemented!()
    }

    fn read_to_string(&self, _path: &str) -> io::Result<String> {
        unimplemented!()
    }

    fn exists(&self, _path: &str) -> bool {
        unimplemented!()
    }
}

fn read_limited(_reader: impl io::Read, _limit: u64) -> io::Result<String> {
    unimplemented!()
}

/// One call made through an [`Env`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    Var(String),
    Read(String),
    Exists(String),
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Var(name) => write!(f, "env {name}"),
            Self::Read(path) => write!(f, "read {path}"),
            Self::Exists(path) => write!(f, "stat {path}"),
        }
    }
}

/// Wraps another [`Env`] and remembers every call: `-v` prints the list, and tests use it
/// as the spy that proves what was (not) looked at.
pub struct RecordingEnv<'a> {
    inner: &'a dyn Env,
    accesses: RefCell<Vec<Access>>,
}

impl<'a> RecordingEnv<'a> {
    pub fn new(inner: &'a dyn Env) -> Self {
        Self {
            inner,
            accesses: RefCell::new(Vec::new()),
        }
    }

    pub fn accesses(&self) -> Vec<Access> {
        self.accesses.borrow().clone()
    }
}

impl Env for RecordingEnv<'_> {
    fn var(&self, _name: &str) -> Option<String> {
        unimplemented!()
    }

    fn read_to_string(&self, _path: &str) -> io::Result<String> {
        unimplemented!()
    }

    fn exists(&self, _path: &str) -> bool {
        unimplemented!()
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// An in-memory host: variables, files with contents, and files that exist but cannot
    /// be read.
    #[derive(Default)]
    pub struct FakeEnv {
        vars: BTreeMap<String, String>,
        files: BTreeMap<String, String>,
        unreadable: BTreeSet<String>,
    }

    impl FakeEnv {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_owned(), value.to_owned());
            self
        }

        pub fn file(mut self, path: &str, content: &str) -> Self {
            self.files.insert(path.to_owned(), content.to_owned());
            self
        }

        pub fn unreadable(mut self, path: &str) -> Self {
            self.unreadable.insert(path.to_owned());
            self
        }
    }

    impl Env for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }

        fn read_to_string(&self, path: &str) -> io::Result<String> {
            if self.unreadable.contains(path) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn exists(&self, path: &str) -> bool {
            self.files.contains_key(path) || self.unreadable.contains(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::testing::FakeEnv;
    use super::*;

    const TOKEN: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

    #[test]
    fn the_allow_list_is_exactly_what_detection_needs() {
        assert_eq!(
            ALLOWED_PATHS,
            [
                "/.dockerenv",
                "/run/.containerenv",
                "/run/systemd/container",
                "/proc/self/cgroup",
                "/proc/self/mountinfo",
                "/proc/net/route",
                "/proc/net/ipv6_route",
                "/etc/resolv.conf",
                "/var/run/secrets/kubernetes.io/serviceaccount/namespace",
            ]
        );
        assert_eq!(
            ALLOWED_VARS,
            [
                "KUBERNETES_SERVICE_HOST",
                "KUBERNETES_SERVICE_PORT",
                "POD_NAME",
                "POD_NAMESPACE",
                "POD_IP",
                "POD_IPS",
                "NODE_NAME",
                "HOST_IP",
            ]
        );
    }

    #[test]
    fn service_account_credentials_are_never_allowed() {
        for path in [
            TOKEN,
            "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt",
            "/run/secrets/kubernetes.io/serviceaccount/token",
            "/var/run/secrets/kubernetes.io/serviceaccount/..data/token",
            "/var/run/secrets/kubernetes.io/serviceaccount/namespace/../token",
            "/var/run/secrets/kubernetes.io/serviceaccount/../serviceaccount/token",
            "/var/run/secrets/kubernetes.io/serviceaccount/token/",
            "/var/run/secrets/kubernetes.io/serviceaccount/",
            "/var/run/secrets/kubernetes.io/serviceaccount",
            "/var/run/secrets/kubernetes.io/serviceaccount/Namespace",
            "/etc/shadow",
            "/proc/self/environ",
            "etc/resolv.conf",
            "/etc/resolv.conf/",
            "//etc/resolv.conf",
            "",
        ] {
            assert!(!SystemEnv::allows_path(path), "{path:?}");
        }
        for path in ALLOWED_PATHS {
            assert!(SystemEnv::allows_path(path), "{path}");
        }
    }

    #[test]
    fn a_refused_path_fails_before_the_filesystem_is_consulted() {
        // `/etc/passwd` exists on every Unix host; the answer must not depend on that.
        for path in [TOKEN, "/etc/passwd", "/"] {
            let err = SystemEnv.read_to_string(path).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{path}");
            assert!(!SystemEnv.exists(path), "{path}");
        }
    }

    #[test]
    fn variables_outside_the_list_read_as_unset() {
        // PATH is set in every test run; it must still not be visible.
        for name in ["PATH", "HOME", "AWS_SECRET_ACCESS_KEY", "pod_ip", ""] {
            assert_eq!(SystemEnv.var(name), None, "{name}");
        }
        assert!(SystemEnv::allows_var("POD_IP"));
        assert!(!SystemEnv::allows_var("PATH"));
    }

    #[test]
    fn reads_are_capped_and_never_fail_on_bad_utf8() {
        let text = read_limited(Cursor::new(b"abcdef".to_vec()), 4).unwrap();
        assert_eq!(text, "abcd");
        let text = read_limited(Cursor::new(vec![b'a', 0xff, b'b']), 10).unwrap();
        assert_eq!(text, "a\u{fffd}b");
        assert_eq!(read_limited(Cursor::new(Vec::new()), 10).unwrap(), "");
    }

    #[test]
    fn the_recording_env_passes_answers_through_and_logs_every_call_in_order() {
        let fake = FakeEnv::new()
            .var("POD_IP", "10.0.0.1")
            .file("/etc/resolv.conf", "nameserver 10.96.0.10\n")
            .unreadable("/proc/self/cgroup");
        let env = RecordingEnv::new(&fake);

        assert_eq!(env.var("POD_IP").as_deref(), Some("10.0.0.1"));
        assert_eq!(env.var("POD_NAME"), None);
        assert!(!env.exists("/.dockerenv"));
        assert!(env.exists("/proc/self/cgroup"));
        assert_eq!(
            env.read_to_string("/etc/resolv.conf").unwrap(),
            "nameserver 10.96.0.10\n"
        );
        assert!(env.read_to_string("/proc/self/cgroup").is_err());

        assert_eq!(
            env.accesses(),
            [
                Access::Var("POD_IP".to_owned()),
                Access::Var("POD_NAME".to_owned()),
                Access::Exists("/.dockerenv".to_owned()),
                Access::Exists("/proc/self/cgroup".to_owned()),
                Access::Read("/etc/resolv.conf".to_owned()),
                Access::Read("/proc/self/cgroup".to_owned()),
            ]
        );
    }

    #[test]
    fn accesses_render_for_verbose_output() {
        assert_eq!(Access::Var("POD_IP".to_owned()).to_string(), "env POD_IP");
        assert_eq!(Access::Read("/a".to_owned()).to_string(), "read /a");
        assert_eq!(Access::Exists("/a".to_owned()).to_string(), "stat /a");
    }
}
