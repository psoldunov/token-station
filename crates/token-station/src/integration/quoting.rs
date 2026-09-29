//! Quoting the binary's path for the three file formats `setup` writes.
//!
//! All three split `Exec=`/`ExecStart=` into words and understand double quotes,
//! and there it stops: each reader has its own idea of which characters need a
//! backslash, which ones are doubled, and whether the value it reads has already
//! been through a string-level unescaping pass. Getting that wrong writes a file
//! that starts the wrong program (or nothing at all) for a path holding a space,
//! a per-cent sign or a quote, so the rules live here, one [`Rules`] per format.

/// How one format wants a quoted word written.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// Characters that take a backslash inside the double quotes.
    pub escaped: &'static [char],
    /// Characters the reader treats as a prefix, so a literal one is doubled.
    pub doubled: &'static [char],
    /// Whether the file itself unescapes backslash sequences before the command
    /// line is split, so every backslash needs one more of its own.
    pub two_level: bool,
}

/// The Desktop Entry spec: `Exec=` is read twice over.
///
/// The value first goes through the spec's string escaping (`\\` is one
/// backslash), and only then is the command line split, where a backslash, a
/// double quote, a backtick and a dollar sign are reserved inside the quotes and
/// `%` starts a field code. A literal `"` therefore reaches the file as `\\"`.
pub const DESKTOP: Rules = Rules {
    escaped: &['"', '`', '$', '\\'],
    doubled: &['%'],
    two_level: true,
};

/// systemd: one unescaping pass, `%%` for a literal per-cent sign and `$$` for a
/// literal dollar sign, which would otherwise start a variable reference.
pub const SYSTEMD: Rules = Rules {
    escaped: &['"', '\\'],
    doubled: &['%', '$'],
    two_level: false,
};

/// A D-Bus session `.service` file: `Exec=` is split the same shell-ish way, but
/// `dbus-daemon` has no specifiers at all — a `%` doubled here would reach the
/// kernel as two per-cent signs and the daemon would not start.
pub const DBUS: Rules = Rules {
    escaped: &['"', '\\'],
    doubled: &[],
    two_level: false,
};

/// Wrap `value` in double quotes, following `rules`.
pub fn quoted(value: &str, rules: &Rules) -> String {
    let mut inner = String::with_capacity(value.len());
    for ch in value.chars() {
        if rules.escaped.contains(&ch) {
            inner.push('\\');
            inner.push(ch);
        } else if rules.doubled.contains(&ch) {
            inner.push(ch);
            inner.push(ch);
        } else {
            inner.push(ch);
        }
    }
    if rules.two_level {
        inner = inner.replace('\\', "\\\\");
    }
    format!("\"{inner}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Undo `\c` escapes inside the quotes and `c`-doubling, the way a reader
    /// following `rules` does. `two_level` files get the string pass first.
    fn unquoted(text: &str, rules: &Rules) -> String {
        let once = if rules.two_level {
            unescape_backslashes(text)
        } else {
            text.to_string()
        };
        let body = once
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(&once);
        split_word(body, rules)
    }

    /// The Desktop Entry string pass: `\\` is one backslash, and nothing else in
    /// a path we write is an escape sequence.
    fn unescape_backslashes(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' => out.push(chars.next().unwrap_or('\\')),
                other => out.push(other),
            }
        }
        out
    }

    /// The command-line pass: a backslash escapes the next character, and a
    /// doubled prefix character stands for one of itself.
    fn split_word(body: &str, rules: &Rules) -> String {
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                out.push(chars.next().unwrap_or('\\'));
            } else if rules.doubled.contains(&ch) && chars.peek() == Some(&ch) {
                chars.next();
                out.push(ch);
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// Paths that exercise every character any of the three formats reserves.
    const AWKWARD: [&str; 7] = [
        "/opt/Token Station.AppImage",
        "/opt/100%.AppImage",
        "/opt/$HOME/token-station",
        r#"/opt/Token "Station".AppImage"#,
        r"/opt/back\slash",
        "/opt/`backtick`",
        r#"/opt/all of it 50% "x" $y `z` \w"#,
    ];

    #[test]
    fn every_format_reads_back_exactly_what_was_quoted() {
        for rules in [&DESKTOP, &SYSTEMD, &DBUS] {
            for path in AWKWARD {
                let written = quoted(path, rules);
                assert_eq!(unquoted(&written, rules), path, "{written}");
            }
        }
    }

    #[test]
    fn a_plain_path_only_gains_the_quotes() {
        for rules in [&DESKTOP, &SYSTEMD, &DBUS] {
            assert_eq!(
                quoted("/opt/Token Station", rules),
                "\"/opt/Token Station\""
            );
        }
    }

    #[test]
    fn the_desktop_entry_spec_needs_both_levels_of_escaping() {
        // `\"` at the command-line level, and the backslash escaped again for
        // the string level the `.desktop` reader applies first.
        assert_eq!(quoted(r#"/opt/a"b"#, &DESKTOP), r#""/opt/a\\"b""#);
        assert_eq!(quoted(r"/opt/a\b", &DESKTOP), r#""/opt/a\\\\b""#);
        assert_eq!(quoted("/opt/a$b", &DESKTOP), r#""/opt/a\\$b""#);
        assert_eq!(quoted("/opt/a`b", &DESKTOP), r#""/opt/a\\`b""#);
        assert_eq!(quoted("/opt/100%", &DESKTOP), "\"/opt/100%%\"");
    }

    #[test]
    fn systemd_doubles_the_per_cent_and_the_dollar_sign() {
        assert_eq!(quoted("/opt/100%", &SYSTEMD), "\"/opt/100%%\"");
        assert_eq!(quoted("/opt/$HOME", &SYSTEMD), "\"/opt/$$HOME\"");
        assert_eq!(quoted(r#"/opt/a"b\c"#, &SYSTEMD), r#""/opt/a\"b\\c""#);
        // A backtick is an ordinary character to systemd.
        assert_eq!(quoted("/opt/`x`", &SYSTEMD), "\"/opt/`x`\"");
    }

    #[test]
    fn the_dbus_activation_file_doubles_nothing() {
        // `dbus-daemon` passes `%` and `$` through verbatim; doubling either
        // would hand the kernel a path that does not exist.
        assert_eq!(quoted("/opt/100%", &DBUS), "\"/opt/100%\"");
        assert_eq!(quoted("/opt/$HOME", &DBUS), "\"/opt/$HOME\"");
        assert_eq!(quoted(r#"/opt/a"b\c"#, &DBUS), r#""/opt/a\"b\\c""#);
    }
}
