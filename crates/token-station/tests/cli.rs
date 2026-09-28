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
    run_with(args, stdin, home, &[])
}

/// As [`run`], with extra environment variables.
fn run_with(args: &[&str], stdin: &str, home: &std::path::Path, extra: &[(&str, &str)]) -> Run {
    let mut command = Command::new(binary());
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        // No bus: the statusline command must fall back to the drop box.
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/ts-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in extra {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("binary runs");
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
fn no_subcommand_outside_an_appimage_prints_the_help() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&[], "", dir.path());
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("Usage: token-station"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("setup"), "{}", out.stdout);
    // Printing help installs nothing.
    assert!(!dir.path().join("state/token-station").exists());
}

#[test]
fn no_subcommand_inside_an_appimage_runs_setup() {
    let dir = tempfile::tempdir().unwrap();
    // `/bin/true` stands in for the AppImage: setup starts it as the tray.
    // An empty PATH keeps the test away from the real `systemctl --user`.
    let out = run_with(
        &[],
        "",
        dir.path(),
        &[("APPIMAGE", "/bin/true"), ("PATH", "")],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert!(
        out.stdout.contains("Token Station is set up"),
        "{}",
        out.stdout
    );

    let manifest = dir.path().join("state/token-station/install-manifest.json");
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    assert_eq!(recorded["exec"], "/bin/true");
    assert_eq!(recorded["desktop"], "other");
    assert!(
        dir.path()
            .join("data/dbus-1/services/dev.soldunov.TokenStation.service")
            .exists()
    );
    assert!(
        dir.path()
            .join("config/autostart/dev.soldunov.TokenStation.Tray.desktop")
            .exists()
    );

    // And `uninstall` takes it all back out again.
    let gone = run_with(&["uninstall"], "", dir.path(), &[("PATH", "")]);
    assert_eq!(gone.code, 0, "{}", gone.stdout);
    assert!(!manifest.exists());
    assert!(
        !dir.path()
            .join("config/autostart")
            .join("dev.soldunov.TokenStation.Tray.desktop")
            .exists()
    );
}

#[test]
fn a_dry_run_prints_the_plan_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_with(
        &["setup", "--desktop", "other", "--dry-run"],
        "",
        dir.path(),
        &[("PATH", "")],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert!(out.stdout.contains("dry run"), "{}", out.stdout);
    assert!(
        out.stdout.contains("token-station.service"),
        "{}",
        out.stdout
    );
    assert!(!dir.path().join("data").exists());
    assert!(!dir.path().join("config").exists());
}

#[test]
fn setup_for_kde_without_a_payload_fails_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_with(
        &["setup", "--desktop", "kde"],
        "",
        dir.path(),
        &[("PATH", "")],
    );
    assert_eq!(out.code, 1);
    assert!(!dir.path().join("data/plasma").exists());
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
