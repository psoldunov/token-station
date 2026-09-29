//! The systemd user unit that runs the daemon.
//!
//! The unit text comes from `data/systemd/token-station.service.in`, the same
//! file the Nix package substitutes, with `@bindir@/token-station` replaced by
//! the quoted path of the binary that is running `setup`.

use std::path::{Path, PathBuf};

use crate::integration::step::Step;
use crate::integration::{Dirs, quoting};

/// Unit file name.
pub const UNIT: &str = "token-station.service";
/// The tool that loads and starts it, when the session has systemd.
pub const SYSTEMCTL: &str = "systemctl";
/// Placeholder the packaged template uses for the install prefix.
const BINDIR_PLACEHOLDER: &str = "@bindir@/token-station";

const TEMPLATE: &str = include_str!("../../../../data/systemd/token-station.service.in");

/// Where the unit is installed.
#[must_use]
pub fn unit_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("systemd/user").join(UNIT)
}

/// Quote one word of a systemd command line; see [`quoting::SYSTEMD`].
#[must_use]
pub fn quote_word(value: &str) -> String {
    quoting::quoted(value, &quoting::SYSTEMD)
}

/// The unit text for `exec`.
#[must_use]
pub fn unit_text(exec: &Path) -> String {
    TEMPLATE.replace(BINDIR_PLACEHOLDER, &quote_word(&exec.to_string_lossy()))
}

/// Write the unit, then reload and enable it when `systemctl` is available.
///
/// The reload and the enable hang off the write ([`Step::Unit`]) rather than
/// following it: when the unit already belongs to a package or to home-manager
/// the write is skipped, and `enable --now` would then start — and `uninstall`
/// would later `disable --now` — a unit that was never ours.
#[must_use]
pub fn steps(dirs: &Dirs, exec: &Path, systemctl: Option<&Path>) -> Vec<Step> {
    let then = match systemctl {
        Some(tool) => vec![
            run(tool, &["daemon-reload"]),
            run(tool, &["enable", "--now", UNIT]),
        ],
        None => vec![Step::Note(format!(
            "`{SYSTEMCTL}` was not found: the daemon will start through D-Bus activation instead."
        ))],
    };
    vec![Step::Unit {
        target: unit_path(dirs),
        contents: unit_text(exec),
        then,
    }]
}

fn run(tool: &Path, args: &[&str]) -> Step {
    Step::Run {
        program: tool.to_path_buf(),
        args: std::iter::once("--user".to_string())
            .chain(args.iter().map(|arg| (*arg).to_string()))
            .collect(),
    }
}

/// Stop and forget the unit, for `uninstall`.
///
/// The reload comes second: systemd has to be told the file is gone, or the unit
/// stays in its list as `not-found` until the next login.
#[must_use]
pub fn disable_steps(systemctl: Option<&Path>) -> Vec<Step> {
    match systemctl {
        Some(tool) => vec![
            run(tool, &["disable", "--now", UNIT]),
            run(tool, &["daemon-reload"]),
        ],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        Dirs {
            home: "/home/u".into(),
            data: "/da".into(),
            config: "/cfg".into(),
            state: "/st".into(),
        }
    }

    #[test]
    fn the_unit_is_a_dbus_service_pointing_at_the_quoted_binary() {
        let text = unit_text(Path::new("/apps/Token Station.AppImage"));
        assert!(
            text.contains("ExecStart=\"/apps/Token Station.AppImage\" daemon"),
            "{text}"
        );
        assert!(text.contains("Type=dbus"));
        assert!(text.contains("BusName=dev.soldunov.TokenStation"));
        assert!(text.contains("WantedBy=graphical-session.target"));
        assert!(!text.contains("@bindir@"), "no placeholder is left");
    }

    #[test]
    fn the_unit_goes_into_the_user_unit_directory() {
        assert_eq!(
            unit_path(&dirs()),
            Path::new("/cfg/systemd/user/token-station.service")
        );
    }

    /// The unit step's target and its follow-up steps.
    fn unit_step(systemctl: Option<&Path>) -> (PathBuf, Vec<Step>) {
        let steps = steps(&dirs(), Path::new("/x"), systemctl);
        assert_eq!(steps.len(), 1, "one step owns the unit: {steps:?}");
        match steps.into_iter().next() {
            Some(Step::Unit { target, then, .. }) => (target, then),
            other => panic!("expected a unit step, got {other:?}"),
        }
    }

    #[test]
    fn with_systemctl_the_unit_is_reloaded_and_enabled_after_its_own_write() {
        let tool = PathBuf::from("/bin/systemctl");
        let (target, then) = unit_step(Some(&tool));
        assert_eq!(target, unit_path(&dirs()));
        assert_eq!(
            then,
            vec![
                Step::Run {
                    program: tool.clone(),
                    args: vec!["--user".into(), "daemon-reload".into()],
                },
                Step::Run {
                    program: tool,
                    args: vec![
                        "--user".into(),
                        "enable".into(),
                        "--now".into(),
                        UNIT.into()
                    ],
                },
            ]
        );
    }

    #[test]
    fn without_systemctl_the_unit_is_still_written() {
        let (target, then) = unit_step(None);
        assert_eq!(target, unit_path(&dirs()));
        assert!(matches!(&then[..], [Step::Note(text)] if text.contains("D-Bus activation")));
        assert!(disable_steps(None).is_empty());
    }

    #[test]
    fn uninstall_stops_the_unit_and_then_reloads() {
        let tool = PathBuf::from("/bin/systemctl");
        assert_eq!(
            disable_steps(Some(&tool)),
            vec![
                Step::Run {
                    program: tool.clone(),
                    args: vec![
                        "--user".into(),
                        "disable".into(),
                        "--now".into(),
                        UNIT.into()
                    ],
                },
                Step::Run {
                    program: tool,
                    args: vec!["--user".into(), "daemon-reload".into()],
                },
            ]
        );
    }

    #[test]
    fn exec_start_is_quoted_the_way_systemd_reads_it() {
        assert_eq!(quote_word("/opt/Token Station"), "\"/opt/Token Station\"");
        assert_eq!(quote_word(r#"/opt/we"ird\x"#), r#""/opt/we\"ird\\x""#);
        // `%` starts a specifier unless it is doubled.
        assert_eq!(quote_word("/opt/100%.AppImage"), "\"/opt/100%%.AppImage\"");

        let text = unit_text(Path::new("/apps/50% \"off\".AppImage"));
        assert!(
            text.contains(r#"ExecStart="/apps/50%% \"off\".AppImage" daemon"#),
            "{text}"
        );
    }
}
