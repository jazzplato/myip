//! The text `--version` prints.

use std::sync::LazyLock;

/// `0.2.0`, or for a release binary `0.2.0 (f02e182 2026-11-22)`: the short commit and its
/// commit date in UTC. The release workflow passes both in the environment at build time;
/// a build without them (a plain `cargo build`, `cargo install` from crates.io) has no
/// repository to ask and prints the bare version.
pub static VERSION: LazyLock<String> = LazyLock::new(|| {
    render(
        env!("CARGO_PKG_VERSION"),
        option_env!("MYIP_BUILD_COMMIT"),
        option_env!("MYIP_BUILD_DATE"),
    )
});

fn render(version: &str, commit: Option<&str>, date: Option<&str>) -> String {
    match (commit, date) {
        (Some(commit), Some(date)) if is_commit(commit) && is_date(date) => {
            format!("{version} ({commit} {date})")
        }
        _ => version.to_owned(),
    }
}

/// An abbreviated git object name: seven or more lower-case hex digits, at most a full SHA-1.
fn is_commit(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `YYYY-MM-DD`, month 01-12, day 01-31 (the calendar is not checked further).
fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = &bytes[range];
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| part.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && digits(0..4).is_some()
        && digits(5..7).is_some_and(|month| (1..=12).contains(&month))
        && digits(8..10).is_some_and(|day| (1..=31).contains(&day))
}

#[cfg(test)]
mod tests {
    use super::render;

    const COMMIT: &str = "f02e182";
    const DATE: &str = "2026-11-22";

    #[test]
    fn a_commit_and_a_date_follow_the_version() {
        assert_eq!(
            render("0.2.0", Some(COMMIT), Some(DATE)),
            "0.2.0 (f02e182 2026-11-22)"
        );
    }

    #[test]
    fn a_longer_commit_is_kept() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        for commit in ["0123456789ab", full] {
            assert_eq!(
                render("0.2.0", Some(commit), Some(DATE)),
                format!("0.2.0 ({commit} {DATE})")
            );
        }
    }

    #[test]
    fn without_both_values_it_is_the_bare_version() {
        for (commit, date) in [
            (None, None),
            (Some(COMMIT), None),
            (None, Some(DATE)),
            (Some(""), Some("")),
            (Some(COMMIT), Some("")),
            (Some(""), Some(DATE)),
        ] {
            assert_eq!(
                render("0.2.0", commit, date),
                "0.2.0",
                "{commit:?} {date:?}"
            );
        }
    }

    #[test]
    fn a_malformed_commit_is_dropped_with_the_date() {
        let too_long = "0".repeat(41);
        for commit in [
            "f02e18",            // shorter than git's minimum of seven
            "F02E182",           // not lower case
            "f02e18g",           // not hex
            "f02e182 2026",      // a space
            "f02e182\n",         // a newline
            "f02e182\u{1b}[31m", // an escape
            too_long.as_str(),
        ] {
            assert_eq!(
                render("0.2.0", Some(commit), Some(DATE)),
                "0.2.0",
                "{commit:?}"
            );
        }
    }

    #[test]
    fn a_malformed_date_is_dropped_with_the_commit() {
        for date in [
            "2026-13-01",
            "2026-00-10",
            "2026-11-32",
            "2026-11-00",
            "2026/11/22",
            "22-11-2026",
            "2026-11-2",
            "2026-1-22",
            "2026-11-22T00:00",
            "2026-11-22\n",
            "20261122",
            "٢٠٢٦-١١-٢٢",
        ] {
            assert_eq!(
                render("0.2.0", Some(COMMIT), Some(date)),
                "0.2.0",
                "{date:?}"
            );
        }
    }
}
