//! The binary's own behaviour: statusline fallback, wrapping and exit codes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SAMPLE: &str = r#"{"model":{"display_name":"Opus 5.5"},"rate_limits":{"five_hour":{"used_percentage":34},"seven_day":{"used_percentage":15}}}"#;

fn binary() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_token-station"))
}

struct Run {
    stdout: String,
    code: i32,
}

/// Run the binary with an isolated environment and no session bus.
fn run(args: &[&str], stdin: &str, home: &std::path::Path) -> Run {
    let mut child = Command::new(binary())
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        // No bus: the statusline command must fall back to the drop box.
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/ts-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("stdin written");
    let output = child.wait_with_output().expect("binary finished");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        code: output.status.code().unwrap_or(-1),
    }
}

#[test]
fn statusline_prints_a_compact_line_and_drops_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["statusline"], SAMPLE, dir.path());
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.trim(), "5h 34% · 7d 15%");

    let dropped = dir.path().join("run/token-station/claude-statusline.json");
    assert_eq!(std::fs::read_to_string(dropped).unwrap(), SAMPLE);
}

#[test]
fn statusline_wrap_passes_stdin_through_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["statusline", "--wrap", "cat"], SAMPLE, dir.path());
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, SAMPLE);
}

#[test]
fn statusline_survives_empty_and_unparsable_input() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(run(&["statusline"], "", dir.path()).code, 0);
    let out = run(&["statusline"], "not json", dir.path());
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.trim(), "");
}

#[test]
fn unfinished_subcommands_exit_with_two() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["tray", "setup", "uninstall"] {
        assert_eq!(run(&[name], "", dir.path()).code, 2, "{name}");
    }
}

#[test]
fn refresh_without_a_daemon_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(run(&["refresh"], "", dir.path()).code, 1);
}

#[test]
fn help_and_version_work() {
    let dir = tempfile::tempdir().unwrap();
    let help = run(&["--help"], "", dir.path());
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("statusline"), "{}", help.stdout);
}

#[test]
fn status_without_a_daemon_still_answers() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config/token-station/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // Both providers off, so the command touches no credentials and no network.
    std::fs::write(
        &config,
        "[claude]\nenabled = false\n[codex]\nenabled = false\n",
    )
    .unwrap();

    let out = run(&["status"], "", dir.path());
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("disabled"), "{}", out.stdout);

    let json = run(&["status", "--json"], "", dir.path());
    assert_eq!(json.code, 0);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("valid JSON");
    assert_eq!(value["schemaVersion"], 1);
}
