//! End-to-end tests driving [`ts_codex::CodexProvider`] through a fake `codex`
//! binary (a `sh` script replaying canned app-server responses, one per
//! stdin line actually received) and a wiremock HTTP server for the fallback
//! path.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use ts_codex::{CodexEnv, CodexProvider, HttpTimeouts};
use ts_core::Provider;
use ts_core::config::{CodexConfig, CodexProcessMode};
use ts_core::discovery::SearchEnv;
use ts_core::pricing::shared_bundled;
use ts_core::snapshot::ProviderState;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Stamp `id` onto a canned response body (the script prints these
/// statically, so the id must already match what the client will request).
fn with_id(mut value: Value, id: i64) -> Value {
    value["id"] = json!(id);
    value
}

/// One entry: consume `consume` stdin lines (the request that triggered this
/// reply, plus any notification lines sent before it), then print `response`
/// (with its own `"id"`, set by the caller to match what will be requested).
type Step = (usize, Value);

fn step(consume: usize, response: Value) -> Step {
    (consume, response)
}

/// A `sh` script that consumes stdin one request at a time and replies with
/// the matching canned `steps`, then idles forever (so `kill_on_drop` — not
/// the script exiting on its own — is what ends it, like a real app-server).
fn write_fake_codex(dir: &Path, name: &str, steps: &[Step], spawn_log: &Path) -> PathBuf {
    let path = dir.join(name);
    let mut script = format!("#!/bin/sh\necho spawned >> '{}'\n", spawn_log.display());
    for (consume, response) in steps {
        for _ in 0..*consume {
            script.push_str("IFS= read -r _line || exit 0\n");
        }
        writeln!(
            script,
            "printf '%s\\n' '{}'",
            serde_json::to_string(response).unwrap()
        )
        .expect("writing to a String never fails");
    }
    // Explicit `<&0` matters: a bare `cat &` gets its stdin silently
    // redirected from /dev/null by a non-interactive shell and exits at
    // once, which would make `wait` return immediately instead of keeping
    // this process alive like a real app-server child.
    script.push_str("cat <&0 >/dev/null &\nwait\n");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A script that exits immediately without answering anything (simulates a crash).
fn write_crashing_codex(dir: &Path, name: &str, spawn_log: &Path) -> PathBuf {
    let path = dir.join(name);
    let script = format!(
        "#!/bin/sh\necho spawned >> '{}'\nexit 1\n",
        spawn_log.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn base_env(dir: &Path, binary: PathBuf) -> CodexEnv {
    CodexEnv {
        home: dir.to_path_buf(),
        codex_home: None,
        binary_override: Some(binary),
        http_base_url: "http://127.0.0.1:1".to_string(), // unused unless a test overrides it
        http_timeouts: HttpTimeouts::default(),
        search: SearchEnv {
            // The real `PATH`, because the fake `codex` is a shell script that
            // runs `cat`, and the child's `PATH` is built from this one — as it
            // is in production, where `SearchEnv::current` always carries it.
            // Discovery itself is not using it: `binary_override` names the
            // script outright.
            path: std::env::var("PATH").ok(),
            home: dir.to_path_buf(),
            user: None,
        },
    }
}

/// A default-config provider backed by a fake `codex` replaying `steps`.
fn provider_with_steps(dir: &Path, spawn_log: &Path, steps: &[Step]) -> CodexProvider {
    let binary = write_fake_codex(dir, "codex", steps, spawn_log);
    CodexProvider::with_env(
        CodexConfig::default(),
        shared_bundled(),
        base_env(dir, binary),
    )
}

fn init_step() -> Step {
    step(1, fixture("initialize_response.json"))
}

/// One `account/read` round: 2 stdin lines the first time (the `initialized`
/// notification plus the request itself), 1 line on every later reuse.
fn account_step(consume: usize, body: Value) -> Step {
    step(consume, body)
}

fn apikey_account_body() -> Value {
    json!({"result": {"account": {"type": "apiKey"}, "requiresOpenaiAuth": true}})
}

#[tokio::test]
async fn not_installed_when_no_binary_and_no_home() {
    // `ts_core::discovery`'s SYSTEM_DIRS (e.g. `/run/current-system/sw/bin`)
    // are fixed, not injectable via `SearchEnv`, so a machine with `codex`
    // actually installed system-wide will always resolve a binary here.
    // There is no hermetic way to force "not found" from ts-codex alone.
    if ts_core::discovery::find_binary_in(
        "codex",
        "",
        &SearchEnv {
            path: None,
            home: PathBuf::from("/nonexistent"),
            user: None,
        },
    )
    .is_some()
    {
        eprintln!("skipping: this machine has a system-wide `codex` binary");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let env = CodexEnv {
        home: dir.path().join("missing-home"),
        codex_home: None,
        binary_override: None,
        http_base_url: "http://127.0.0.1:1".into(),
        http_timeouts: HttpTimeouts::default(),
        search: SearchEnv {
            path: Some(String::new()),
            home: dir.path().to_path_buf(),
            user: None,
        },
    };
    let provider = CodexProvider::with_env(CodexConfig::default(), shared_bundled(), env);
    let outcome = provider.refresh_limits(true).await;
    assert!(matches!(outcome, ts_core::RefreshOutcome::Skipped(_)));
    assert_eq!(provider.snapshot(0).state, ProviderState::NotInstalled);
}

#[tokio::test]
async fn api_key_account_reports_no_windows() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let steps = [
        init_step(),
        account_step(2, with_id(apikey_account_body(), 2)),
    ];
    let provider = provider_with_steps(dir.path(), &spawn_log, &steps);

    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    let snapshot = provider.snapshot(0);
    assert_eq!(snapshot.state, ProviderState::Ok);
    assert_eq!(snapshot.plan.as_deref(), Some("API key"));
    assert!(snapshot.windows.is_empty());
}

#[tokio::test]
async fn unauthenticated_account_reports_message() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let unauth = with_id(
        json!({"result": {"account": null, "requiresOpenaiAuth": true}}),
        2,
    );
    let steps = [init_step(), account_step(2, unauth)];
    let provider = provider_with_steps(dir.path(), &spawn_log, &steps);

    provider.refresh_limits(true).await;
    let snapshot = provider.snapshot(0);
    assert_eq!(snapshot.state, ProviderState::Unauthenticated);
    assert!(snapshot.message.unwrap().contains("codex login"));
}

#[tokio::test]
async fn chatgpt_account_maps_windows_credits_and_account_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let steps = [
        init_step(),
        account_step(2, fixture("account_read_response.json")),
        step(1, fixture("rate_limits_read_response.json")),
        step(1, fixture("usage_read_response.json")),
    ];
    let provider = provider_with_steps(dir.path(), &spawn_log, &steps);

    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    let snapshot = provider.snapshot(0);
    assert_eq!(snapshot.state, ProviderState::Ok);
    assert_eq!(snapshot.plan.as_deref(), Some("Pro"));
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].id, "codex:primary");
    assert_eq!(snapshot.windows[0].source, "app-server");
    let account_tokens = snapshot.account_tokens.unwrap();
    assert_eq!(account_tokens.lifetime, Some(4_743_603_614));
    let credits = snapshot.credits.unwrap();
    assert_eq!(credits.detail, Some("1 reset credit(s) available".into()));
}

#[tokio::test]
async fn unsupported_method_falls_back_to_http() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let unsupported = with_id(
        json!({"error": {"code": -32600, "message": "unknown variant `account/read`"}}),
        2,
    );
    let steps = [init_step(), account_step(2, unsupported)];
    let binary = write_fake_codex(dir.path(), "codex", &steps, &spawn_log);

    // `resolve_homes()` defaults to `<home>/.codex` when `config.homes` and
    // `$CODEX_HOME` are unset, so auth.json must live there for the fallback
    // to find it.
    let codex_home = dir.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        codex_home.join("auth.json"),
        include_str!("fixtures/auth.json"),
    )
    .unwrap();

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/backend-api/wham/usage"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "plan_type": "plus",
            "rate_limit": {"primary_window": {"used_percent": 3.0, "limit_window_seconds": 604_800}}
        })))
        .mount(&server)
        .await;

    let mut env = base_env(dir.path(), binary);
    env.http_base_url = server.uri();
    let provider = CodexProvider::with_env(CodexConfig::default(), shared_bundled(), env);

    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    let snapshot = provider.snapshot(0);
    assert_eq!(snapshot.state, ProviderState::Ok);
    assert_eq!(snapshot.plan.as_deref(), Some("Plus"));
    assert_eq!(snapshot.windows[0].source, "http");
}

#[tokio::test]
async fn missing_account_usage_read_keeps_windows_and_plan() {
    // `account/usage/read` is optional: an app-server that returns -32600
    // for it must not lose the windows/credits/plan already fetched from
    // `account/rateLimits/read`.
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let unsupported_usage = with_id(
        json!({"error": {"code": -32600, "message": "unknown variant `account/usage/read`"}}),
        1,
    );
    let steps = [
        init_step(),
        account_step(2, fixture("account_read_response.json")),
        step(1, fixture("rate_limits_read_response.json")),
        step(1, unsupported_usage),
    ];
    let provider = provider_with_steps(dir.path(), &spawn_log, &steps);

    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    let snapshot = provider.snapshot(0);
    assert_eq!(snapshot.state, ProviderState::Ok);
    assert_eq!(snapshot.plan.as_deref(), Some("Pro"));
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].id, "codex:primary");
    assert!(snapshot.account_tokens.is_none());
}

#[tokio::test]
async fn crashing_process_yields_error_when_no_fallback_available() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let binary = write_crashing_codex(dir.path(), "codex", &spawn_log);
    let config = CodexConfig {
        http_fallback: false,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    let outcome = provider.refresh_limits(true).await;
    assert!(matches!(outcome, ts_core::RefreshOutcome::Failed(_)));
    assert_eq!(provider.snapshot(0).state, ProviderState::Error);
}

#[tokio::test]
async fn on_demand_reuses_session_within_linger() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    // One shared app-server client across all three refreshes: request ids
    // keep incrementing (2, 3, 4) instead of resetting each round.
    let steps = [
        init_step(),
        account_step(2, with_id(apikey_account_body(), 2)),
        account_step(1, with_id(apikey_account_body(), 3)),
        account_step(1, with_id(apikey_account_body(), 4)),
    ];
    let binary = write_fake_codex(dir.path(), "codex", &steps, &spawn_log);
    let config = CodexConfig {
        linger_secs: 3600,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    for _ in 0..3 {
        let outcome = provider.refresh_limits(true).await;
        assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    }

    let spawns = fs::read_to_string(&spawn_log).unwrap_or_default();
    assert_eq!(
        spawns.lines().count(),
        1,
        "expected exactly one spawn, got: {spawns}"
    );
}

#[tokio::test]
async fn on_demand_respawns_after_linger_elapses() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    // Each respawn is a fresh client and fresh script run, so ids restart at 1/2 every time.
    let steps = [
        init_step(),
        account_step(2, with_id(apikey_account_body(), 2)),
    ];
    let binary = write_fake_codex(dir.path(), "codex", &steps, &spawn_log);
    let config = CodexConfig {
        process_mode: CodexProcessMode::OnDemand,
        linger_secs: 0,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    provider.refresh_limits(true).await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    provider.refresh_limits(true).await;

    let spawns = fs::read_to_string(&spawn_log).unwrap_or_default();
    assert_eq!(
        spawns.lines().count(),
        2,
        "expected a respawn, got: {spawns}"
    );
}

#[tokio::test]
async fn http_fallback_never_touches_auth_json() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    let auth_path = codex_home.join("auth.json");
    fs::write(&auth_path, include_str!("fixtures/auth.json")).unwrap();
    let before_bytes = fs::read(&auth_path).unwrap();
    let before_mtime = fs::metadata(&auth_path).unwrap().modified().unwrap();

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    // No real `codex` binary: force the app-server attempt to fail fast so
    // this actually exercises the HTTP fallback's read of `auth.json`.
    let mut env = base_env(
        dir.path(),
        PathBuf::from("/definitely/does/not/exist/codex"),
    );
    env.http_base_url = server.uri();
    let provider = CodexProvider::with_env(CodexConfig::default(), shared_bundled(), env);
    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);

    assert_eq!(fs::read(&auth_path).unwrap(), before_bytes);
    assert_eq!(
        fs::metadata(&auth_path).unwrap().modified().unwrap(),
        before_mtime
    );
}

#[tokio::test]
async fn next_limits_refresh_backs_off_after_failure_then_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let binary = write_crashing_codex(dir.path(), "codex", &spawn_log);
    let config = CodexConfig {
        http_fallback: false,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    let default_interval = std::time::Duration::from_secs(300);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap();
    assert_eq!(
        provider.next_limits_refresh(now, default_interval),
        default_interval
    );

    provider.refresh_limits(true).await;
    let backed_off = provider.next_limits_refresh(now, default_interval);
    assert!(
        backed_off >= std::time::Duration::from_secs(60),
        "expected the 60s initial backoff, got {backed_off:?}"
    );
    assert!(
        provider.next_limits_refresh(now + 10_000, default_interval)
            >= std::time::Duration::from_secs(30)
    );
}

#[tokio::test]
#[expect(
    clippy::float_cmp,
    reason = "compares exact literals that never went through arithmetic"
)]
async fn rollout_rate_limits_are_used_as_last_resort() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let codex_home = dir.path().join(".codex");
    let rollout = codex_home.join("sessions/2026/09/28/rollout-a.jsonl");
    fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    fs::write(
        &rollout,
        concat!(
            r#"{"timestamp":"2026-09-28T12:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"#,
            r#""rate_limits":{"limit_id":"codex","plan_type":"pro","primary":{"used_percent":42.0,"window_minutes":10080,"resets_at":1791199753}}}}"#,
            "\n"
        ),
    )
    .unwrap();

    let binary = write_crashing_codex(dir.path(), "codex", &spawn_log);
    let config = CodexConfig {
        http_fallback: false,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    provider.refresh_tokens().await;
    let outcome = provider.refresh_limits(true).await;
    assert_eq!(outcome, ts_core::RefreshOutcome::Updated);
    let snapshot = provider.snapshot(0);
    // Rollout-derived data is shown as `Stale`, never `Ok`: it can be
    // arbitrarily old, and `updated_at` reflects the rollout line's own
    // timestamp rather than "now".
    assert_eq!(snapshot.state, ProviderState::Stale);
    assert_eq!(snapshot.updated_at, Some(1_790_596_800)); // 2026-09-28T12:00:00Z
    assert_eq!(snapshot.windows[0].source, "rollout");
    assert_eq!(snapshot.windows[0].used_percent, 42.0);
    assert!(snapshot.message.unwrap().contains("local session logs"));
}

/// Runs the real `codex app-server` once. Requires `codex` on `PATH` and a
/// signed-in `~/.codex`. Never run in CI; operators run it manually:
/// `TS_LIVE=1 cargo test -p ts-codex --test provider -- --ignored live_app_server_round_trip`
#[tokio::test]
#[ignore = "runs the real `codex app-server`; opt in with TS_LIVE=1"]
async fn live_app_server_round_trip() {
    if std::env::var("TS_LIVE").as_deref() != Ok("1") {
        return;
    }
    let provider = CodexProvider::new(CodexConfig::default(), shared_bundled());
    let outcome = provider.refresh_limits(true).await;
    println!("live outcome: {outcome:?}");
    let snapshot = provider.snapshot(0);
    println!(
        "live state: {:?}, plan: {:?}",
        snapshot.state, snapshot.plan
    );
    assert_ne!(snapshot.state, ProviderState::Loading);
}

/// Is `pid` a live process, rather than gone or a zombie waiting to be reaped?
///
/// Not `kill(pid, 0)`, which a zombie still answers: what this asks is whether
/// the child is still *running*, and a killed-but-unreaped child is not.
#[cfg(target_os = "linux")]
fn process_alive(pid: &str) -> bool {
    fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
        !status
            .lines()
            .any(|l| l.starts_with("State:") && l.contains('Z'))
    })
}

/// As above, through `ps`: macOS has no `/proc`.
///
/// `/bin/ps` by absolute path, because the test runs with whatever `PATH` the
/// build gave it.
#[cfg(not(target_os = "linux"))]
fn process_alive(pid: &str) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-o", "state=", "-p", pid])
        .output()
        .is_ok_and(|output| {
            let state = String::from_utf8_lossy(&output.stdout);
            let state = state.trim();
            !state.is_empty() && !state.starts_with('Z')
        })
}

#[tokio::test]
async fn on_demand_stops_idle_child_after_linger() {
    let dir = tempfile::tempdir().unwrap();
    let spawn_log = dir.path().join("spawns.log");
    let pid_file = dir.path().join("pid");
    let steps = [
        init_step(),
        account_step(2, with_id(apikey_account_body(), 2)),
    ];
    let binary = write_fake_codex(dir.path(), "codex", &steps, &spawn_log);
    let script = fs::read_to_string(&binary).unwrap().replacen(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\necho $$ > '{}'\n", pid_file.display()),
        1,
    );
    fs::write(&binary, script).unwrap();
    let config = CodexConfig {
        process_mode: CodexProcessMode::OnDemand,
        linger_secs: 1,
        ..CodexConfig::default()
    };
    let provider = CodexProvider::with_env(config, shared_bundled(), base_env(dir.path(), binary));

    assert_eq!(
        provider.refresh_limits(true).await,
        ts_core::RefreshOutcome::Updated
    );
    let pid = fs::read_to_string(&pid_file).unwrap().trim().to_string();
    assert!(
        process_alive(&pid),
        "child should linger right after a refresh"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_800)).await;
    assert!(
        !process_alive(&pid),
        "idle child should be stopped after linger_secs"
    );
}
