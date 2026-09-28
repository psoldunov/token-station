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

/// `Some(inner)` when `command` already is a Token Station status line; `inner`
/// is whatever it was wrapping. `None` when the command belongs to someone else.
pub fn parse_ours(command: &str) -> Option<Option<String>> {
    let words = tokenize(command);
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
    let mut backup = settings.as_os_str().to_os_string();
    backup.push(BACKUP_SUFFIX);
    Ok(Plan {
        settings: settings.to_path_buf(),
        backup: PathBuf::from(backup),
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
    let previous = match (was_ours, recorded) {
        (true, Some(record)) => record.previous.clone(),
        _ => status_line(&members),
    };

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
            parse_ours("'/x' statusline --wrap 'ccusage statusline'"),
            Some(Some("ccusage statusline".into()))
        );
        assert_eq!(parse_ours("starship prompt"), None);
        assert_eq!(parse_ours(""), None);
        assert_eq!(parse_ours("token-station"), None);
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
    fn a_symlinked_settings_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("store-settings.json");
        std::fs::write(&real, "{}").unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let error = plan(&link, Path::new(EXEC)).unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
        assert!(error.contains("home-manager"), "{error}");
        // Nothing was touched.
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{}");
    }

    #[test]
    fn a_missing_or_read_only_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("settings.json");
        assert!(
            plan(&missing, Path::new(EXEC))
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );

        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = settings_with("{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(
            plan(&path, Path::new(EXEC))
                .unwrap_err()
                .to_string()
                .contains("not writable")
        );
    }

    #[test]
    fn restore_puts_back_what_was_there() {
        let (_dir, path) = settings_with(
            r#"{"model": "opus", "statusLine": {"type": "command", "command": "starship"}}"#,
        );
        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        assert!(restore(&record).unwrap());
        assert_eq!(read(&path)["statusLine"]["command"], "starship");
        assert_eq!(read(&path)["model"], "opus");
    }

    #[test]
    fn restore_removes_the_key_when_there_was_none() {
        let (_dir, path) = settings_with(r#"{"model": "opus"}"#);
        let record = apply(&plan(&path, Path::new(EXEC)).unwrap(), None).unwrap();
        assert!(restore(&record).unwrap());
        assert!(read(&path).get("statusLine").is_none());
        assert_eq!(read(&path)["model"], "opus");
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
