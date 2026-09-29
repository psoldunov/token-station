//! `--claude-statusline`: point Claude Code's `statusLine` at this binary.
//!
//! The file belongs to the user, so the rules are strict: only a regular,
//! writable `settings.json` is touched, a symlink (Nix or home-manager owns it)
//! is refused, every other key keeps its place and its order, and an existing
//! status line is wrapped rather than replaced — once, never twice.
//!
//! Order and formatting survive because the file is read as an ordered map of
//! untouched raw JSON: only the `statusLine` member is ever rewritten.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use indexmap::IndexMap;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::atomic::write_atomic;
use crate::integration::manifest::ClaudeRecord;
use crate::paths::Env;

/// Suffix of the copy taken before the first patch.
pub const BACKUP_SUFFIX: &str = ".token-station-backup";

/// What the patch will do, decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub settings: PathBuf,
    pub backup: PathBuf,
    /// The `statusLine.command` we are going to write.
    pub command: String,
}

/// The copy taken next to `settings` before the first patch.
pub fn backup_path(settings: &Path) -> PathBuf {
    let mut backup = settings.as_os_str().to_os_string();
    backup.push(BACKUP_SUFFIX);
    PathBuf::from(backup)
}

/// `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
pub fn settings_path(env: &Env, config_dir: Option<&str>) -> PathBuf {
    match config_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("settings.json"),
        None => env.home_dir().join(".claude/settings.json"),
    }
}

/// Wrap `value` in single quotes, POSIX style.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Split a command line into words, honouring quotes and backslash escapes.
fn tokenize(command: &str) -> Vec<String> {
    let (mut words, mut word) = (Vec::new(), String::new());
    let (mut quote, mut escaped, mut quoted) = (None, false, false);
    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_some() {
            word.push(ch);
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            quoted = true;
        } else if ch.is_whitespace() {
            if quoted || !word.is_empty() {
                words.push(std::mem::take(&mut word));
                quoted = false;
            }
        } else {
            word.push(ch);
        }
    }
    if quoted || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Is `program` this binary, or an AppImage of it?
///
/// `<something> statusline` is not enough on its own: `ccusage statusline` is a
/// different tool with the same subcommand, and treating it as ours would drop the
/// user's status line instead of wrapping it.
fn is_our_program(program: &str) -> bool {
    let Some(name) = Path::new(program).file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == "token-station"
        || lower == "token_station"
        || (lower.ends_with(".appimage")
            && lower.replace(['-', '_'], "").starts_with("tokenstation"))
}

/// `Some(inner)` when `command` already is a Token Station status line; `inner`
/// is whatever it was wrapping. `None` when the command belongs to someone else.
pub fn parse_ours(command: &str) -> Option<Option<String>> {
    let words = tokenize(command);
    if !words.first().is_some_and(|program| is_our_program(program)) {
        return None;
    }
    if words.get(1).map(String::as_str) != Some("statusline") {
        return None;
    }
    let wrapped = words
        .iter()
        .position(|word| word == "--wrap")
        .and_then(|at| words.get(at + 1))
        .cloned();
    Some(wrapped)
}

/// `'<exec>' statusline [--wrap '<wrapped>']`.
pub fn build_command(exec: &Path, wrapped: Option<&str>) -> String {
    let base = format!("{} statusline", quote(&exec.to_string_lossy()));
    match wrapped {
        Some(inner) => format!("{base} --wrap {}", quote(inner)),
        None => base,
    }
}

/// Top-level members of `settings.json`, in file order, values untouched.
type Members = IndexMap<String, Box<RawValue>>;

/// The `statusLine` value as a parsed JSON value.
fn status_line(members: &Members) -> Option<Value> {
    serde_json::from_str(members.get("statusLine")?.get()).ok()
}

/// The `statusLine.command` currently configured, if it is a command hook.
fn current_command(members: &Members) -> Option<String> {
    let line = status_line(members)?;
    if line.get("type")?.as_str()? != "command" {
        return None;
    }
    line.get("command")?.as_str().map(str::to_string)
}

/// Refuse anything that is not a regular, writable file the user owns.
fn check_writable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::symlink_metadata(path).with_context(|| {
        format!(
            "{} does not exist; start Claude Code once (or create the file) and re-run",
            path.display()
        )
    })?;
    if meta.file_type().is_symlink() {
        bail!(
            "{} is a symlink, so it is managed elsewhere (Nix or home-manager); \
             add the status line to that configuration instead",
            path.display()
        );
    }
    if !meta.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    if meta.permissions().mode() & 0o200 == 0 {
        bail!("{} is not writable", path.display());
    }
    Ok(())
}

/// Decide the patch without writing anything.
pub fn plan(settings: &Path, exec: &Path) -> anyhow::Result<Plan> {
    check_writable(settings)?;
    let members = read_object(settings)?;
    let previous = current_command(&members);
    // Already ours: keep wrapping whatever it wrapped, never wrap ourselves.
    let wrapped = match previous.as_deref().and_then(parse_ours) {
        Some(inner) => inner,
        None => previous,
    };
    Ok(Plan {
        settings: settings.to_path_buf(),
        backup: backup_path(settings),
        command: build_command(exec, wrapped.as_deref()),
    })
}

/// Parse `settings.json` into its top-level members, insisting on an object.
fn read_object(path: &Path) -> anyhow::Result<Members> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        bail!("{} does not contain a JSON object", path.display());
    }
    serde_json::from_str(trimmed).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Turn a value back into the raw JSON the file stores.
fn raw(value: &Value) -> anyhow::Result<Box<RawValue>> {
    Ok(RawValue::from_string(serde_json::to_string(value)?)?)
}

/// What `uninstall` should put back once this patch is applied.
///
/// When the status line is already ours but no manifest says what it replaced (the
/// state directory was cleared, say), our own command is the one thing that must
/// never be stored: `uninstall` would then "restore" Token Station's own status
/// line and the user would be left with it forever. The command we wrap is the
/// best record of what was there before, and nothing at all is the honest answer
/// when we wrap nothing.
fn previous_value(
    members: &Members,
    was_ours: bool,
    recorded: Option<&ClaudeRecord>,
) -> Option<Value> {
    match (was_ours, recorded) {
        (true, Some(record)) => record.previous.clone(),
        (true, None) => wrapped_command(members)
            .map(|inner| serde_json::json!({ "type": "command", "command": inner })),
        (false, _) => status_line(members),
    }
}

/// The command our own status line wraps, if it is ours and it wraps one.
fn wrapped_command(members: &Members) -> Option<String> {
    parse_ours(&current_command(members)?).flatten()
}

/// Apply the patch. `recorded` is the manifest entry from an earlier run, whose
/// `previous` value survives every re-run.
pub fn apply(plan: &Plan, recorded: Option<&ClaudeRecord>) -> anyhow::Result<ClaudeRecord> {
    check_writable(&plan.settings)?;
    let mut members = read_object(&plan.settings)?;
    let was_ours = current_command(&members)
        .as_deref()
        .and_then(parse_ours)
        .is_some();
    if !was_ours {
        std::fs::copy(&plan.settings, &plan.backup)
            .with_context(|| format!("cannot back up {}", plan.settings.display()))?;
    }
    let previous = previous_value(&members, was_ours, recorded);

    members.insert(
        "statusLine".into(),
        raw(&serde_json::json!({ "type": "command", "command": plan.command }))?,
    );
    write_json(&plan.settings, &members)?;
    Ok(ClaudeRecord {
        settings: plan.settings.clone(),
        backup: plan.backup.clone(),
        previous,
        command: plan.command.clone(),
    })
}

/// Put back what was there before, unless someone changed it since.
///
/// Returns `false` when the current value is no longer ours and was left alone.
pub fn restore(record: &ClaudeRecord) -> anyhow::Result<bool> {
    // The file may have become a symlink or gone read-only since `setup` ran.
    check_writable(&record.settings)?;
    let mut members = read_object(&record.settings)?;
    if current_command(&members).as_deref() != Some(record.command.as_str()) {
        return Ok(false);
    }
    match &record.previous {
        Some(value) => {
            members.insert("statusLine".into(), raw(value)?);
        }
        None => {
            members.shift_remove("statusLine");
        }
    }
    write_json(&record.settings, &members)?;
    Ok(true)
}

/// Rewrite the file. [`write_atomic`] carries the old permission bits over, so a
/// `settings.json` the user kept at `0600` is not widened to `0644`.
fn write_json(path: &Path, members: &Members) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(members)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const EXEC: &str = "/opt/TokenStation.AppImage";

    fn settings_with(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_settings_file_follows_claude_config_dir() {
        let env = Env::from_map(&HashMap::from([("HOME", "/home/u")]));
        assert_eq!(
            settings_path(&env, None),
            Path::new("/home/u/.claude/settings.json")
        );
        assert_eq!(
            settings_path(&env, Some("/cfg/claude")),
            Path::new("/cfg/claude/settings.json")
        );
        // An empty variable is not a directory.
        assert_eq!(
            settings_path(&env, Some("  ")),
            Path::new("/home/u/.claude/settings.json")
        );
    }

    #[test]
    fn commands_are_quoted_so_spaces_survive() {
        assert_eq!(
            build_command(Path::new("/opt/Token Station.AppImage"), None),
            "'/opt/Token Station.AppImage' statusline"
        );
        assert_eq!(
            build_command(Path::new(EXEC), Some("starship prompt")),
            "'/opt/TokenStation.AppImage' statusline --wrap 'starship prompt'"
        );
        // A quote in the wrapped command does not end the quoting.
        let tricky = build_command(Path::new(EXEC), Some("echo it's fine"));
        assert_eq!(parse_ours(&tricky), Some(Some("echo it's fine".into())));
    }

    #[test]
    fn our_own_command_is_recognised_and_nobody_elses() {
        assert_eq!(
            parse_ours("'/opt/TokenStation.AppImage' statusline"),
            Some(None)
        );
        assert_eq!(
            parse_ours("'/usr/bin/token-station' statusline --wrap 'ccusage statusline'"),
            Some(Some("ccusage statusline".into()))
        );
        assert_eq!(parse_ours("starship prompt"), None);
        assert_eq!(parse_ours(""), None);
        assert_eq!(parse_ours("token-station"), None);
    }

    #[test]
    fn another_tools_statusline_subcommand_is_not_ours() {
        // Wrapping these would be right; claiming them is how the user's own
        // status line gets thrown away on the next `setup`.
        assert_eq!(parse_ours("ccusage statusline"), None);
        assert_eq!(parse_ours("/usr/bin/ccusage statusline --json"), None);
        assert_eq!(parse_ours("'bunx ccusage' statusline"), None);
        assert_eq!(parse_ours("npx some-tool statusline"), None);
    }

    #[test]
    fn our_program_is_matched_by_name_wherever_it_lives() {
        assert!(is_our_program("/usr/bin/token-station"));
        assert!(is_our_program("token-station"));
        assert!(is_our_program("/opt/TokenStation.AppImage"));
        assert!(is_our_program("/apps/TokenStation-x86_64.AppImage"));
        assert!(is_our_program("/apps/token-station-0.1.0-x86_64.AppImage"));
        assert!(!is_our_program("ccusage"));
        assert!(!is_our_program("/opt/Other.AppImage"));
        assert!(!is_our_program(""));
    }

    #[test]
    fn a_restricted_settings_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = settings_with(r#"{"model": "opus"}"#);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "0600 must not widen to 0644");
        assert!(restore(&record).unwrap());
        assert_eq!(mode(&path), 0o600, "restore must not widen it either");
    }

    #[test]
    fn restore_refuses_a_file_that_became_managed_elsewhere() {
        let (dir, path) = settings_with(r#"{"model": "opus"}"#);
        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();

        // Someone put the file under Nix (or home-manager) after `setup` ran.
        let real = dir.path().join("store-settings.json");
        std::fs::copy(&path, &real).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();

        let error = restore(&record).unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
    }

    #[test]
    fn a_pristine_file_is_backed_up_and_every_other_key_keeps_its_order() {
        let (_dir, path) =
            settings_with(r#"{"zebra": 1, "model": "opus", "apple": {"nested": true}}"#);
        let plan = plan(&path, Path::new(EXEC)).unwrap();
        let record = apply(&plan, None).unwrap();

        assert_eq!(record.previous, None);
        assert_eq!(
            read(&path)["statusLine"]["command"],
            "'/opt/TokenStation.AppImage' statusline"
        );
        // The original file is preserved verbatim next to it.
        assert_eq!(
            std::fs::read_to_string(&record.backup).unwrap(),
            r#"{"zebra": 1, "model": "opus", "apple": {"nested": true}}"#
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let order: Vec<&str> = ["zebra", "model", "apple", "statusLine"]
            .into_iter()
            .filter(|key| text.contains(&format!("\"{key}\"")))
            .collect();
        assert_eq!(order, vec!["zebra", "model", "apple", "statusLine"]);
        assert!(
            text.find("\"zebra\"") < text.find("\"model\"")
                && text.find("\"model\"") < text.find("\"apple\""),
            "{text}"
        );
    }

    #[test]
    fn nested_objects_keep_their_own_order_and_formatting() {
        let original = "{\n  \"permissions\": {\n    \"zebra\": true,\n    \"apple\": false\n  },\n  \"model\": \"opus\"\n}\n";
        let (_dir, path) = settings_with(original);
        apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"zebra\": true,\n    \"apple\": false"),
            "the nested object was rewritten:\n{text}"
        );
        assert!(text.find("\"permissions\"") < text.find("\"model\""));
        assert!(text.find("\"model\"") < text.find("\"statusLine\""));
    }

    #[test]
    fn an_existing_status_line_is_wrapped_exactly_once() {
        let (_dir, path) =
            settings_with(r#"{"statusLine": {"type": "command", "command": "starship prompt"}}"#);
        let first = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        assert_eq!(
            first.command,
            "'/opt/TokenStation.AppImage' statusline --wrap 'starship prompt'"
        );

        // Re-running (say, after the AppImage moved) rewrites the path and keeps
        // one level of wrapping and the original previous value.
        let moved = Path::new("/apps/TokenStation.AppImage");
        let second = apply(&plan(&path, moved).unwrap(), Some(&first)).unwrap();
        assert_eq!(
            second.command,
            "'/apps/TokenStation.AppImage' statusline --wrap 'starship prompt'"
        );
        assert_eq!(second.previous, first.previous);
        assert_eq!(
            second.previous.as_ref().unwrap()["command"],
            "starship prompt"
        );
    }

    #[test]
    fn a_file_we_must_not_rewrite_is_refused_with_a_reason() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();

        let target = dir.path().join("store-settings.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("linked.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let read_only = dir.path().join("read-only.json");
        std::fs::write(&read_only, "{}").unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o444)).unwrap();

        for (path, expected) in [
            (&link, "symlink"),
            (&link, "home-manager"),
            (&read_only, "not writable"),
            (&dir.path().join("absent.json"), "does not exist"),
        ] {
            let error = plan(path, Path::new(EXEC)).unwrap_err().to_string();
            assert!(error.contains(expected), "{expected} missing from: {error}");
        }
        // Nothing was touched on the way through.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}");
        assert_eq!(std::fs::read_to_string(&read_only).unwrap(), "{}");
    }

    #[test]
    fn restore_puts_back_exactly_what_was_there_before() {
        // A status line that existed comes back; one that did not is removed again.
        let (_dir, had_one) = settings_with(
            r#"{"model": "opus", "statusLine": {"type": "command", "command": "starship"}}"#,
        );
        let (_dir2, had_none) = settings_with(r#"{"model": "opus"}"#);
        for path in [&had_one, &had_none] {
            let record = apply(&plan(path, Path::new(EXEC)).unwrap(), None).unwrap();
            assert!(restore(&record).unwrap(), "{}", path.display());
            assert_eq!(read(path)["model"], "opus", "every other key survives");
        }
        assert_eq!(read(&had_one)["statusLine"]["command"], "starship");
        assert!(read(&had_none).get("statusLine").is_none());
    }

    #[test]
    fn restore_leaves_a_status_line_someone_else_changed() {
        let (_dir, path) = settings_with(r#"{}"#);
        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        std::fs::write(
            &path,
            r#"{"statusLine": {"type": "command", "command": "mine now"}}"#,
        )
        .unwrap();
        assert!(!restore(&record).unwrap());
        assert_eq!(read(&path)["statusLine"]["command"], "mine now");
    }

    #[test]
    fn our_own_command_is_never_stored_as_the_thing_to_restore() {
        // A re-run with no manifest left (the state directory was cleared): the
        // wrapped command is what `uninstall` must put back, never our own.
        let ours = "'/opt/TokenStation.AppImage' statusline --wrap 'starship'";
        let (_dir, wrapping) = settings_with(&format!(
            r#"{{"statusLine": {{"type": "command", "command": {}}}}}"#,
            serde_json::to_string(ours).unwrap()
        ));
        let record = apply(&plan(&wrapping, Path::new(EXEC)).unwrap(), None).unwrap();
        assert_eq!(
            record.previous,
            Some(serde_json::json!({"type": "command", "command": "starship"}))
        );
        assert!(restore(&record).unwrap());
        assert_eq!(read(&wrapping)["statusLine"]["command"], "starship");

        // And with nothing wrapped, there is nothing to put back.
        let bare = "'/opt/TokenStation.AppImage' statusline";
        let (_dir2, plain) = settings_with(&format!(
            r#"{{"statusLine": {{"type": "command", "command": {}}}}}"#,
            serde_json::to_string(bare).unwrap()
        ));
        let record = apply(&plan(&plain, Path::new(EXEC)).unwrap(), None).unwrap();
        assert_eq!(record.previous, None);
        assert!(restore(&record).unwrap());
        assert!(read(&plain).get("statusLine").is_none());
    }

    #[test]
    fn a_non_command_status_line_is_preserved_whole() {
        let (_dir, path) = settings_with(r#"{"statusLine": {"type": "static", "text": "hi"}}"#);
        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        // Nothing to wrap: a non-command hook is not a shell command.
        assert_eq!(record.command, "'/opt/TokenStation.AppImage' statusline");
        assert_eq!(record.previous.as_ref().unwrap()["type"], "static");
        assert!(restore(&record).unwrap());
        assert_eq!(read(&path)["statusLine"]["text"], "hi");
    }

    #[test]
    fn a_settings_file_that_is_not_an_object_is_refused() {
        let (_dir, path) = settings_with("[1, 2, 3]");
        assert!(
            plan(&path, Path::new(EXEC))
                .unwrap_err()
                .to_string()
                .contains("JSON object")
        );
        let (_dir2, broken) = settings_with("{oops");
        assert!(
            plan(&broken, Path::new(EXEC))
                .unwrap_err()
                .to_string()
                .contains("not valid JSON")
        );
    }
}
