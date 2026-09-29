//! The session-bus activation file, so any client call starts the daemon.
//!
//! Built from `data/dbus/dev.soldunov.TokenStation.service.in` with the install
//! prefix replaced by the quoted binary. `SystemdService=` hands the process to
//! systemd when the session has it; the template carries the line already, so it
//! is only appended when a template without it is in use.

use std::path::{Path, PathBuf};

use crate::integration::step::Step;
use crate::integration::systemd::UNIT;
use crate::integration::{Dirs, quoting};

/// File name, which must match the bus name.
pub const SERVICE_FILE: &str = "dev.soldunov.TokenStation.service";
const BINDIR_PLACEHOLDER: &str = "@bindir@/token-station";

const TEMPLATE: &str = include_str!("../../../../data/dbus/dev.soldunov.TokenStation.service.in");

/// Where the activation file is installed.
pub fn service_path(dirs: &Dirs) -> PathBuf {
    dirs.data.join("dbus-1/services").join(SERVICE_FILE)
}

/// The `SystemdService=` key, written exactly once.
const SYSTEMD_KEY: &str = "SystemdService=";

/// Quote one word of the `Exec=` line; see [`quoting::DBUS`].
///
/// Deliberately not systemd's quoting: `dbus-daemon` has no specifiers, so a `%`
/// doubled for systemd's benefit would be passed through as two per-cent signs and
/// the activation would fail on any path containing one.
pub fn quote_exec(value: &str) -> String {
    quoting::quoted(value, &quoting::DBUS)
}

/// The activation file for `exec`.
pub fn service_text(exec: &Path) -> String {
    let quoted = quote_exec(&exec.to_string_lossy());
    let mut text = TEMPLATE.replace(BINDIR_PLACEHOLDER, &quoted);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    // A second key would be read as a duplicate and the file rejected.
    if !text.lines().any(|line| line.starts_with(SYSTEMD_KEY)) {
        text.push_str(&format!("{SYSTEMD_KEY}{UNIT}\n"));
    }
    text
}

/// Write the activation file.
pub fn steps(dirs: &Dirs, exec: &Path) -> Vec<Step> {
    vec![Step::Write {
        target: service_path(dirs),
        contents: service_text(exec),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::BUS_NAME;

    fn dirs() -> Dirs {
        Dirs {
            home: "/home/u".into(),
            data: "/da".into(),
            config: "/cfg".into(),
            state: "/st".into(),
        }
    }

    #[test]
    fn the_file_sits_where_the_session_bus_looks_for_it() {
        assert_eq!(
            service_path(&dirs()),
            Path::new("/da/dbus-1/services/dev.soldunov.TokenStation.service")
        );
        assert!(SERVICE_FILE.starts_with(BUS_NAME));
    }

    #[test]
    fn the_exec_line_is_quoted_and_systemd_owns_the_process_once() {
        let text = service_text(Path::new("/apps/Token Station.AppImage"));
        assert!(
            text.contains("Exec=\"/apps/Token Station.AppImage\" daemon"),
            "{text}"
        );
        assert!(text.contains(&format!("Name={BUS_NAME}")), "{text}");
        assert!(!text.contains("@bindir@"), "{text}");
        // The template already carries the key; a second one voids the file.
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with(SYSTEMD_KEY))
                .count(),
            1,
            "{text}"
        );
        assert!(text.contains(&format!("{SYSTEMD_KEY}{UNIT}")), "{text}");
    }

    /// `dbus-daemon` reads this line itself when the session has no systemd, and it
    /// expands nothing: the `%` and `$` systemd would want doubled must reach the
    /// file exactly as the path spells them, or the daemon never starts.
    /// [`quoting`](crate::integration::quoting) round-trips the rules themselves.
    #[test]
    fn the_exec_line_does_not_use_systemds_escaping() {
        let text = service_text(Path::new("/apps/50% $x \"off\".AppImage"));
        assert!(
            text.contains(r#"Exec="/apps/50% $x \"off\".AppImage" daemon"#),
            "{text}"
        );
    }

    #[test]
    fn the_only_step_is_the_write() {
        assert_eq!(
            steps(&dirs(), Path::new("/x")),
            vec![Step::Write {
                target: service_path(&dirs()),
                contents: service_text(Path::new("/x")),
            }]
        );
    }
}
