#![cfg(not(target_os = "macos"))]
//! A private session bus for the D-Bus integration tests.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// A `dbus-daemon --session` that lives as long as this value.
pub struct PrivateBus {
    child: Child,
    pub address: String,
}

impl PrivateBus {
    /// Start a private bus, or `None` when `dbus-daemon` is not installed.
    pub fn start() -> Option<PrivateBus> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
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
