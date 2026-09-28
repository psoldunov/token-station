//! Quoting the binary's path for the two file formats `setup` writes.
//!
//! The Desktop Entry `Exec=` key and a systemd `ExecStart=` line use the same
//! shape — double quotes, backslash escapes inside them, `%%` for a literal
//! per-cent sign — and differ only in which characters need the backslash.

/// Characters the Desktop Entry spec wants escaped inside `Exec="…"`.
pub const DESKTOP_ESCAPED: [char; 4] = ['"', '`', '$', '\\'];
/// Characters systemd wants escaped inside a quoted command-line word.
pub const SYSTEMD_ESCAPED: [char; 2] = ['"', '\\'];

/// Wrap `value` in double quotes, backslashing each of `escaped` and doubling `%`.
///
/// A bare `%` starts a field code in a `.desktop` file and a specifier in a unit,
/// so a path such as `/opt/100%.AppImage` has to be written `100%%`.
pub fn quoted(value: &str, escaped: &[char]) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        if escaped.contains(&ch) {
            out.push('\\');
            out.push(ch);
        } else if ch == '%' {
            out.push_str("%%");
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_only_gains_the_quotes() {
        assert_eq!(
            quoted("/opt/Token Station", &DESKTOP_ESCAPED),
            "\"/opt/Token Station\""
        );
    }

    #[test]
    fn each_format_escapes_its_own_characters() {
        let awkward = r#"/opt/we"ird`$\x"#;
        assert_eq!(
            quoted(awkward, &DESKTOP_ESCAPED),
            r#""/opt/we\"ird\`\$\\x""#
        );
        // systemd leaves a backtick and a dollar sign alone.
        assert_eq!(quoted(awkward, &SYSTEMD_ESCAPED), r#""/opt/we\"ird`$\\x""#);
    }

    #[test]
    fn a_per_cent_sign_is_doubled_in_both() {
        assert_eq!(quoted("100%", &DESKTOP_ESCAPED), "\"100%%\"");
        assert_eq!(quoted("100%", &SYSTEMD_ESCAPED), "\"100%%\"");
    }
}
