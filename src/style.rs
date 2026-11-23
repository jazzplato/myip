//! Colour for the text reports of `local`, `where`, `all` and a `--verify` disagreement
//! (ADR 0002). Colour is cosmetic: with the codes removed the text is the plain text, byte
//! for byte, so every span here wraps words that stay.

use crate::cli::ColorChoice;
use crate::sanitize;

/// What the process knows about stdout and what the environment says about colour. The one
/// place that reads those variables: they only decide cosmetics, so they are not part of the
/// detection surface (`Env`) and never show up in the `-v` trace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terminal {
    stdout_is_terminal: bool,
    windows: bool,
    no_color: bool,
    force: bool,
    clicolor_off: bool,
    dumb: bool,
    ansi_console: bool,
}

impl Terminal {
    /// The real process.
    pub fn detect() -> Self {
        Self::default()
    }

    /// The facts from a lookup of environment variables; tests pass fixtures.
    pub fn from_lookup(
        stdout_is_terminal: bool,
        windows: bool,
        var: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let _ = var;
        Self {
            stdout_is_terminal,
            windows,
            ..Self::default()
        }
    }
}

/// Whether colour is on: the decision order of ADR 0002.
pub fn enabled(choice: ColorChoice, terminal: &Terminal) -> bool {
    let _ = terminal;
    choice == ColorChoice::Always
}

/// How text is written: plain, or with ANSI SGR codes around the spans that carry meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Ansi,
}

impl Style {
    pub fn choose(choice: ColorChoice, terminal: &Terminal) -> Self {
        let _ = (choice, terminal);
        Self::Plain
    }

    /// Names of blocks, fields and interfaces.
    pub fn bold(self, text: &str) -> String {
        text.to_owned()
    }

    /// Provenance and explanations.
    pub fn dim(self, text: &str) -> String {
        text.to_owned()
    }

    /// A verified or agreeing answer.
    pub fn good(self, text: &str) -> String {
        text.to_owned()
    }

    /// Unverified, unknown, a guess, a translated address.
    pub fn warn(self, text: &str) -> String {
        text.to_owned()
    }

    /// A disagreement, a failure, no answer.
    pub fn bad(self, text: &str) -> String {
        text.to_owned()
    }

    /// `text` as it may reach stdout: every control character is shown as an escape, except
    /// the newlines and, in colour, our own codes.
    pub fn clean(self, text: &str) -> String {
        sanitize::escape_keeping_newlines(text)
    }
}

/// Test support: the plain text behind a coloured one.
#[cfg(test)]
pub(crate) mod testing {
    /// Every code this module writes, and nothing else.
    const CODES: [&str; 6] = [
        "\u{1b}[0m",
        "\u{1b}[1m",
        "\u{1b}[2m",
        "\u{1b}[31m",
        "\u{1b}[32m",
        "\u{1b}[33m",
    ];

    /// `text` without our SGR codes. Any other escape byte stays, so a test can see it.
    pub fn strip(text: &str) -> String {
        CODES
            .iter()
            .fold(text.to_owned(), |text, code| text.replace(code, ""))
    }

    /// Every line opens and closes its own spans: nothing stays coloured across a newline,
    /// and a span is never nested in another.
    pub fn assert_balanced(text: &str) {
        for line in text.lines() {
            let mut open = false;
            let mut rest = line;
            while let Some(at) = rest.find('\u{1b}') {
                let code = CODES
                    .iter()
                    .find(|code| rest[at..].starts_with(**code))
                    .unwrap_or_else(|| panic!("a code that is not ours in {line:?}"));
                if *code == "\u{1b}[0m" {
                    assert!(open, "a reset without a span in {line:?}");
                    open = false;
                } else {
                    assert!(!open, "nested spans in {line:?}");
                    open = true;
                }
                rest = &rest[at + code.len()..];
            }
            assert!(!open, "a span left open at the end of {line:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ColorChoice::{Always, Auto, Never};

    fn terminal(vars: &[(&str, &str)], tty: bool, windows: bool) -> Terminal {
        Terminal::from_lookup(tty, windows, |name| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    /// (choice, environment, stdout is a terminal, windows, colour, why)
    type Row = (
        ColorChoice,
        &'static [(&'static str, &'static str)],
        bool,
        bool,
        bool,
        &'static str,
    );

    #[test]
    fn the_decision_order_of_adr_0002() {
        let none: &'static [(&str, &str)] = &[];
        let table: &[Row] = &[
            (Always, none, false, false, true, "always colours a pipe"),
            (
                Always,
                &[("NO_COLOR", "1")],
                false,
                false,
                true,
                "the flag beats NO_COLOR",
            ),
            (
                Always,
                &[("TERM", "dumb")],
                false,
                false,
                true,
                "the flag beats TERM=dumb",
            ),
            (
                Always,
                &[("CLICOLOR", "0")],
                true,
                false,
                true,
                "the flag beats CLICOLOR=0",
            ),
            (
                Always,
                none,
                false,
                true,
                true,
                "always colours without a console hint",
            ),
            (Never, none, true, false, false, "never beats a terminal"),
            (
                Never,
                &[("CLICOLOR_FORCE", "1")],
                true,
                false,
                false,
                "never beats CLICOLOR_FORCE",
            ),
            (Auto, none, true, false, true, "a terminal"),
            (Auto, none, false, false, false, "a pipe"),
            (
                Auto,
                &[("NO_COLOR", "1")],
                true,
                false,
                false,
                "NO_COLOR turns it off",
            ),
            (
                Auto,
                &[("NO_COLOR", "")],
                true,
                false,
                true,
                "an empty NO_COLOR is not set",
            ),
            (
                Auto,
                &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
                true,
                false,
                false,
                "NO_COLOR beats CLICOLOR_FORCE",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "1")],
                false,
                false,
                true,
                "CLICOLOR_FORCE colours a pipe",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "yes")],
                false,
                false,
                true,
                "any non-empty value but 0 forces",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "0")],
                false,
                false,
                false,
                "0 does not force",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "0")],
                true,
                false,
                true,
                "0 does not force or forbid: the terminal decides",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "")],
                false,
                false,
                false,
                "an empty CLICOLOR_FORCE is not set",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "1"), ("CLICOLOR", "0")],
                false,
                false,
                true,
                "CLICOLOR_FORCE beats CLICOLOR=0",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "1"), ("TERM", "dumb")],
                false,
                false,
                true,
                "CLICOLOR_FORCE beats TERM=dumb",
            ),
            (
                Auto,
                &[("CLICOLOR", "0")],
                true,
                false,
                false,
                "CLICOLOR=0 turns it off",
            ),
            (
                Auto,
                &[("CLICOLOR", "1")],
                true,
                false,
                true,
                "CLICOLOR=1 is the default",
            ),
            (
                Auto,
                &[("CLICOLOR", "1")],
                false,
                false,
                false,
                "CLICOLOR=1 does not force",
            ),
            (
                Auto,
                &[("TERM", "dumb")],
                true,
                false,
                false,
                "a dumb terminal",
            ),
            (
                Auto,
                &[("TERM", "xterm-256color")],
                true,
                false,
                true,
                "a capable terminal",
            ),
            (Auto, none, true, true, false, "a legacy Windows console"),
            (
                Auto,
                &[("WT_SESSION", "x")],
                true,
                true,
                true,
                "Windows Terminal",
            ),
            (Auto, &[("ANSICON", "1")], true, true, true, "ANSICON"),
            (
                Auto,
                &[("ConEmuANSI", "ON")],
                true,
                true,
                true,
                "ConEmu with ANSI",
            ),
            (
                Auto,
                &[("ConEmuANSI", "OFF")],
                true,
                true,
                false,
                "ConEmu without ANSI",
            ),
            (
                Auto,
                &[("TERM", "xterm")],
                true,
                true,
                true,
                "a TERM on Windows (MSYS, Cygwin)",
            ),
            (
                Auto,
                &[("TERM", "")],
                true,
                true,
                false,
                "an empty TERM is not set",
            ),
            (
                Auto,
                &[("WT_SESSION", "x")],
                false,
                true,
                false,
                "a pipe in Windows Terminal",
            ),
            (
                Auto,
                &[("CLICOLOR_FORCE", "1")],
                false,
                true,
                true,
                "CLICOLOR_FORCE needs no console hint",
            ),
            (
                Auto,
                &[("NO_COLOR", "1"), ("WT_SESSION", "x")],
                true,
                true,
                false,
                "NO_COLOR on Windows",
            ),
            (
                Auto,
                &[("WT_SESSION", "x")],
                true,
                false,
                true,
                "the hint changes nothing elsewhere",
            ),
        ];
        for (choice, vars, tty, windows, expected, why) in table {
            let terminal = terminal(vars, *tty, *windows);
            assert_eq!(
                enabled(*choice, &terminal),
                *expected,
                "{choice:?} {vars:?} tty={tty} windows={windows}: {why}"
            );
            assert_eq!(
                Style::choose(*choice, &terminal),
                if *expected { Style::Ansi } else { Style::Plain },
                "{choice:?} {vars:?} tty={tty} windows={windows}: {why}"
            );
        }
    }

    #[test]
    fn a_process_that_knows_nothing_gets_no_colour_unless_asked() {
        assert!(!enabled(Auto, &Terminal::default()));
        assert!(!enabled(Never, &Terminal::default()));
        assert!(enabled(Always, &Terminal::default()));
    }

    #[test]
    fn a_span_is_a_code_the_words_and_a_reset() {
        assert_eq!(Style::Ansi.bold("x"), "\u{1b}[1mx\u{1b}[0m");
        assert_eq!(Style::Ansi.dim("x"), "\u{1b}[2mx\u{1b}[0m");
        assert_eq!(Style::Ansi.good("x"), "\u{1b}[32mx\u{1b}[0m");
        assert_eq!(Style::Ansi.warn("x"), "\u{1b}[33mx\u{1b}[0m");
        assert_eq!(Style::Ansi.bad("x"), "\u{1b}[31mx\u{1b}[0m");
    }

    #[test]
    fn nothing_is_wrapped_when_there_is_nothing_to_show_or_colour_is_off() {
        for style in [Style::Plain, Style::Ansi] {
            for span in [
                Style::bold,
                Style::dim,
                Style::good,
                Style::warn,
                Style::bad,
            ] {
                assert_eq!(span(style, ""), "", "{style:?}");
            }
        }
        for span in [
            Style::bold,
            Style::dim,
            Style::good,
            Style::warn,
            Style::bad,
        ] {
            assert_eq!(span(Style::Plain, "a b"), "a b");
        }
    }

    #[test]
    fn clean_lets_our_own_codes_through_and_shows_everything_else() {
        let ours = format!(
            "{} and {}\n",
            Style::Ansi.bold("name"),
            Style::Ansi.bad("no")
        );
        assert_eq!(Style::Ansi.clean(&ours), ours);
        for hostile in [
            "\u{1b}[2J",
            "\u{1b}[31",
            "\u{1b}[0;31m",
            "\u{1b}[1;1H",
            "\u{1b}]0;title\u{7}",
            "\u{1b}",
            "\u{9b}31m",
            "\r",
            "\u{202e}",
        ] {
            let text = format!("a{hostile}b\n");
            let cleaned = Style::Ansi.clean(&text);
            assert_eq!(
                cleaned,
                sanitize::escape_keeping_newlines(&text),
                "{hostile:?}"
            );
            assert!(!cleaned.contains('\u{1b}'), "{hostile:?}");
        }
        assert_eq!(
            Style::Ansi.clean(&format!("\u{1b}[2J{ours}")),
            format!("\\x1b[2J{ours}")
        );
    }

    #[test]
    fn plain_text_never_keeps_an_escape_not_even_ours() {
        let ours = Style::Ansi.bold("name");
        assert_eq!(
            Style::Plain.clean(&ours),
            sanitize::escape_keeping_newlines(&ours)
        );
        assert!(!Style::Plain.clean(&ours).contains('\u{1b}'));
    }

    #[test]
    fn the_test_helpers_see_what_they_claim_to() {
        let text = "\u{1b}[1ma\u{1b}[0mx\u{1b}[2mb\u{1b}[0m\n";
        assert_eq!(testing::strip(text), "axb\n");
        testing::assert_balanced(text);
        assert_eq!(testing::strip("\u{1b}[2J"), "\u{1b}[2J");
    }

    #[test]
    #[should_panic(expected = "nested spans")]
    fn nested_spans_are_caught() {
        testing::assert_balanced("\u{1b}[1ma\u{1b}[2mb\u{1b}[0m\u{1b}[0m");
    }

    #[test]
    #[should_panic(expected = "left open")]
    fn a_span_across_a_newline_is_caught() {
        testing::assert_balanced("\u{1b}[1ma\nb\u{1b}[0m");
    }
}
