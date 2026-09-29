//! The binary's own behaviour: statusline fallback, wrapping and exit codes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use token_station::paths::{Env, Paths};

const SAMPLE: &str = r#"{"model":{"display_name":"Opus 5.5"},"rate_limits":{"five_hour":{"used_percentage":34},"seven_day":{"used_percentage":15}}}"#;

fn binary() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_token-station"))
}

struct Run {
    stdout: String,
    code: i32,
}

/// Where a run rooted at `home` keeps its files.
///
/// Asked rather than spelled out, because the layout is the platform's: Linux
/// follows the XDG variables below and macOS ignores them for the Apple
/// directories under `~/Library`.
fn paths_for(home: &Path) -> Paths {
    let at = |suffix: &str| Some(home.join(suffix).to_string_lossy().into_owned());
    Paths::resolve(&Env {
        home: Some(home.to_string_lossy().into_owned()),
        config_home: at("config"),
        state_home: at("state"),
        cache_home: at("cache"),
        runtime_dir: at("run"),
        data_home: at("data"),
        socket: None,
    })
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

    let dropped = paths_for(dir.path()).statusline_drop();
    assert_eq!(std::fs::read_to_string(dropped).unwrap(), SAMPLE);
}

#[test]
fn statusline_wrap_passes_stdin_through_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["statusline", "--wrap", "cat"], SAMPLE, dir.path());
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, SAMPLE);
}

/// A simple invocation of the binary: given `args`/`stdin`, the exit code and
/// (optionally) the stdout it should produce.
struct SimpleCase {
    name: &'static str,
    args: &'static [&'static str],
    stdin: &'static str,
    expect_code: i32,
    stdout_trim_eq: Option<&'static str>,
    stdout_contains: Option<&'static str>,
}

#[test]
fn statusline_edge_cases_and_help_exit_cleanly() {
    let cases = [
        SimpleCase {
            name: "empty stdin still exits cleanly",
            args: &["statusline"],
            stdin: "",
            expect_code: 0,
            stdout_trim_eq: None,
            stdout_contains: None,
        },
        SimpleCase {
            name: "unparsable stdin still exits cleanly",
            args: &["statusline"],
            stdin: "not json",
            expect_code: 0,
            stdout_trim_eq: Some(""),
            stdout_contains: None,
        },
        SimpleCase {
            name: "help prints usage",
            args: &["--help"],
            stdin: "",
            expect_code: 0,
            stdout_trim_eq: None,
            stdout_contains: Some("statusline"),
        },
    ];

    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let out = run(case.args, case.stdin, dir.path());
        assert_eq!(out.code, case.expect_code, "{}: exit code", case.name);
        if let Some(expected) = case.stdout_trim_eq {
            assert_eq!(out.stdout.trim(), expected, "{}: stdout", case.name);
        }
        if let Some(needle) = case.stdout_contains {
            assert!(
                out.stdout.contains(needle),
                "{}: stdout should contain {needle:?}\n{}",
                case.name,
                out.stdout
            );
        }
    }
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
    // `setup` is Linux's; macOS installs an app bundle instead.
    assert_eq!(
        out.stdout.contains("setup"),
        cfg!(not(target_os = "macos")),
        "{}",
        out.stdout
    );
    // Printing help installs nothing.
    assert!(!paths_for(dir.path()).state_dir.exists());
}

/// The desktop integration is Linux's alone.
#[cfg(not(target_os = "macos"))]
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

/// The desktop integration is Linux's alone.
#[cfg(not(target_os = "macos"))]
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

/// The desktop integration is Linux's alone.
#[cfg(not(target_os = "macos"))]
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
fn status_without_a_daemon_still_answers() {
    let dir = tempfile::tempdir().unwrap();
    let config = paths_for(dir.path()).config_file;
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
