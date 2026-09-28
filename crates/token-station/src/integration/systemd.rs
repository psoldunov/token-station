//! The systemd user unit that runs the daemon.
//!
//! The unit text comes from `data/systemd/token-station.service.in`, the same
//! file the Nix package substitutes, with `@bindir@/token-station` replaced by
//! the quoted path of the binary that is running `setup`.

use std::path::{Path, PathBuf};

use crate::integration::Dirs;
use crate::integration::step::Step;

/// Unit file name.
pub const UNIT: &str = "token-station.service";
/// The tool that loads and starts it, when the session has systemd.
pub const SYSTEMCTL: &str = "systemctl";
/// Placeholder the packaged template uses for the install prefix.
const BINDIR_PLACEHOLDER: &str = "@bindir@/token-station";

const TEMPLATE: &str = include_str!("../../../../data/systemd/token-station.service.in");

/// Where the unit is installed.
pub fn unit_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("systemd/user").join(UNIT)
}

/// The unit text for `exec`.
pub fn unit_text(exec: &Path) -> String {
    let quoted = format!("\"{}\"", exec.display());
    TEMPLATE.replace(BINDIR_PLACEHOLDER, &quoted)
}

/// Write the unit, then reload and enable it when `systemctl` is available.
pub fn steps(dirs: &Dirs, exec: &Path, systemctl: Option<&Path>) -> Vec<Step> {
    let mut steps = vec![Step::Write {
        target: unit_path(dirs),
        contents: unit_text(exec),
    }];
    match systemctl {
        Some(tool) => {
            steps.push(run(tool, &["daemon-reload"]));
            steps.push(run(tool, &["enable", "--now", UNIT]));
        }
        None => steps.push(Step::Note(format!(
            "`{SYSTEMCTL}` was not found: the daemon will start through D-Bus activation instead."
        ))),
    }
    steps
}

fn run(tool: &Path, args: &[&str]) -> Step {
    Step::Run {
        program: tool.to_path_buf(),
        args: std::iter::once("--user".to_string())
            .chain(args.iter().map(|arg| (*arg).to_string()))
            .collect(),
    }
}

/// `systemctl --user disable --now token-station.service`, for `uninstall`.
pub fn disable_steps(systemctl: Option<&Path>) -> Vec<Step> {
    match systemctl {
        Some(tool) => vec![run(tool, &["disable", "--now", UNIT])],
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

    #[test]
    fn with_systemctl_the_unit_is_reloaded_and_enabled() {
        let tool = PathBuf::from("/bin/systemctl");
        let steps = steps(&dirs(), Path::new("/x"), Some(&tool));
        assert!(matches!(&steps[0], Step::Write { target, .. } if *target == unit_path(&dirs())));
        assert_eq!(
            steps[1],
            Step::Run {
                program: tool.clone(),
                args: vec!["--user".into(), "daemon-reload".into()],
            }
        );
        assert_eq!(
            steps[2],
            Step::Run {
                program: tool,
                args: vec![
                    "--user".into(),
                    "enable".into(),
                    "--now".into(),
                    UNIT.into()
                ],
            }
        );
    }

    #[test]
    fn without_systemctl_the_unit_is_still_written() {
        let steps = steps(&dirs(), Path::new("/x"), None);
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], Step::Write { .. }));
        assert!(matches!(&steps[1], Step::Note(text) if text.contains("D-Bus activation")));
        assert!(disable_steps(None).is_empty());
    }

    #[test]
    fn uninstall_disables_and_stops_in_one_call() {
        let tool = PathBuf::from("/bin/systemctl");
        assert_eq!(
            disable_steps(Some(&tool)),
            vec![Step::Run {
                program: tool,
                args: vec![
                    "--user".into(),
                    "disable".into(),
                    "--now".into(),
                    UNIT.into()
                ],
            }]
        );
    }
}
