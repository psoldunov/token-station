#![cfg(not(target_os = "macos"))]
//! A private session bus for the D-Bus integration tests.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// A session bus that activates nothing.
///
/// `dbus-daemon --session` reads the system's own session config, whose
/// service directories include every installed `.service` file. On a machine
/// with Token Station installed, a call to its name on the "empty" bus would
/// start the installed daemon; a bus with no service directory cannot.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=@DIR@</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

/// A private `dbus-daemon` that lives as long as this value.
pub struct PrivateBus {
    child: Child,
    pub address: String,
    /// Holds the config and the socket; removed after the bus is gone.
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    /// Start a private bus, or `None` when `dbus-daemon` is not installed.
    pub fn start() -> Option<PrivateBus> {
        let dir = tempfile::tempdir().ok()?;
        let config = dir.path().join("bus.conf");
        let contents = BUS_CONFIG.replace("@DIR@", &dir.path().to_string_lossy());
        std::fs::write(&config, contents).ok()?;
        let mut child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut address = String::new();
        if BufReader::new(stdout).read_line(&mut address).is_err() || address.trim().is_empty() {
            let _ = child.kill();
            return None;
        }
        Some(PrivateBus {
            child,
            address: address.trim().to_string(),
            _dir: dir,
        })
    }

    /// A fresh client connection to this bus.
    pub async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("valid bus address")
            .build()
            .await
            .expect("private bus accepts connections")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Skip the test body (with a note) when no `dbus-daemon` is available.
#[macro_export]
macro_rules! private_bus_or_skip {
    () => {
        match $crate::common::PrivateBus::start() {
            Some(bus) => bus,
            None => {
                eprintln!("skipping: dbus-daemon is not available");
                return;
            }
        }
    };
}
