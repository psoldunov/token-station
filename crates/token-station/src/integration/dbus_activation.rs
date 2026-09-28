//! The session-bus activation file, so any client call starts the daemon.
//!
//! Built from `data/dbus/dev.soldunov.TokenStation.service.in` with the install
//! prefix replaced by the quoted binary, plus `SystemdService=` so systemd owns
//! the process when the session has it.

use std::path::{Path, PathBuf};

use crate::integration::Dirs;
use crate::integration::step::Step;
use crate::integration::systemd::UNIT;

/// File name, which must match the bus name.
pub const SERVICE_FILE: &str = "dev.soldunov.TokenStation.service";
const BINDIR_PLACEHOLDER: &str = "@bindir@/token-station";

const TEMPLATE: &str = include_str!("../../../../data/dbus/dev.soldunov.TokenStation.service.in");

/// Where the activation file is installed.
pub fn service_path(dirs: &Dirs) -> PathBuf {
    dirs.data.join("dbus-1/services").join(SERVICE_FILE)
}

/// The activation file for `exec`.
pub fn service_text(exec: &Path) -> String {
    let quoted = format!("\"{}\"", exec.display());
    let mut text = TEMPLATE.replace(BINDIR_PLACEHOLDER, &quoted);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    // Hand the process to systemd when it is there; harmless when it is not.
    text.push_str(&format!("SystemdService={UNIT}\n"));
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
    fn the_exec_line_is_quoted_and_systemd_owns_the_process() {
        let text = service_text(Path::new("/apps/Token Station.AppImage"));
        assert!(
            text.contains("Exec=\"/apps/Token Station.AppImage\" daemon"),
            "{text}"
        );
        assert!(text.contains(&format!("Name={BUS_NAME}")));
        assert!(
            text.ends_with("SystemdService=token-station.service\n"),
            "{text}"
        );
        assert!(!text.contains("@bindir@"));
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
