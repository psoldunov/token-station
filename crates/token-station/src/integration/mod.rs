//! `token-station setup` and `token-station uninstall`: desktop integration for
//! the portable AppImage.
//!
//! Setup builds a plan, executes it, and writes down exactly what it created in
//! `$XDG_STATE_HOME/token-station/install-manifest.json`. Uninstall reads that
//! manifest and undoes precisely those entries — nothing else is ever removed.

pub mod claude_settings;
pub mod dbus_activation;
pub mod desktop;
pub mod existing;
pub mod gnome;
pub mod manifest;
pub mod plasma;
pub mod quoting;
pub mod removal;
pub mod step;
pub mod systemd;

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::integration::desktop::Desktop;
use crate::integration::existing::{Previous, SystemDirs};
use crate::integration::manifest::{ClaudeRecord, Manifest};
use crate::integration::step::{Outcome, Step};
use crate::paths::Env;

/// Payload location inside an unpacked AppImage, relative to `$APPDIR`.
pub const APPDIR_PAYLOAD: &str = "usr/share/token-station/integrations";

/// The XDG roots `setup` writes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub home: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
}

impl Dirs {
    /// Resolve every root from the environment.
    pub fn resolve(env: &Env) -> Dirs {
        Dirs {
            home: env.home_dir(),
            data: env.data_home(),
            config: env.config_home(),
            state: env.state_home(),
        }
    }
}

/// The non-XDG variables `setup` reads, kept explicit so tests can set them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub current_desktop: Option<String>,
    /// Path of the running `.AppImage`, set by the type2 runtime.
    pub appimage: Option<String>,
    /// Mount point of the running AppImage, set by the type2 runtime.
    pub appdir: Option<String>,
    pub path: Option<String>,
    pub claude_config_dir: Option<String>,
    /// `$DBUS_SESSION_BUS_ADDRESS`; kept explicit so a test never reaches the
    /// developer's own session bus and stops their tray.
    pub bus_address: Option<String>,
    /// System roots searched for a packaged copy of what `setup` installs.
    pub system: SystemDirs,
}

impl Session {
    /// Read the variables from the process environment.
    pub fn current() -> Session {
        let var = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
        Session {
            current_desktop: var("XDG_CURRENT_DESKTOP"),
            appimage: var("APPIMAGE"),
            appdir: var("APPDIR"),
            path: var("PATH"),
            claude_config_dir: var("CLAUDE_CONFIG_DIR"),
            bus_address: var("DBUS_SESSION_BUS_ADDRESS"),
            system: SystemDirs::current(),
        }
    }
}

/// `--desktop`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DesktopChoice {
    #[default]
    Auto,
    Kde,
    Gnome,
    Other,
}

impl DesktopChoice {
    fn resolve(self, session: &Session) -> Desktop {
        match self {
            DesktopChoice::Auto => desktop::detect(session.current_desktop.as_deref()),
            DesktopChoice::Kde => Desktop::Kde,
            DesktopChoice::Gnome => Desktop::Gnome,
            DesktopChoice::Other => Desktop::Other,
        }
    }
}

/// Everything `setup` was asked to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupOptions {
    pub desktop: DesktopChoice,
    pub claude_statusline: bool,
    pub payload_dir: Option<PathBuf>,
    pub dry_run: bool,
    /// Replace files `setup` did not install, and shadow a packaged install.
    pub force: bool,
}

/// The binary the installed files should point at.
pub fn exec_path(session: &Session) -> anyhow::Result<PathBuf> {
    match session.appimage.as_deref() {
        Some(path) => Ok(PathBuf::from(path)),
        None => std::env::current_exe().context("cannot find the path of this binary"),
    }
}

/// Where the front-end payloads live.
pub fn payload_dir(options: &SetupOptions, session: &Session) -> anyhow::Result<PathBuf> {
    if let Some(dir) = &options.payload_dir {
        return Ok(dir.clone());
    }
    match session.appdir.as_deref() {
        Some(appdir) => Ok(Path::new(appdir).join(APPDIR_PAYLOAD)),
        None => anyhow::bail!(
            "no front-end payload: run the AppImage (which sets $APPDIR) or pass --payload-dir"
        ),
    }
}

/// Find an executable on `PATH`; nothing else is searched.
pub fn on_path(name: &str, path_var: Option<&str>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    path_var?
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// A plan, ready to print or to execute.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub desktop: Desktop,
    pub exec: PathBuf,
}

/// Work out everything `setup` would do, without touching anything.
pub fn plan(options: &SetupOptions, env: &Env, session: &Session) -> anyhow::Result<Plan> {
    let dirs = Dirs::resolve(env);
    let exec = exec_path(session)?;
    let target = options.desktop.resolve(session);
    let systemctl = on_path(systemd::SYSTEMCTL, session.path.as_deref());

    let mut steps = match target {
        Desktop::Kde => plasma::steps(&payload_dir(options, session)?, &dirs)?,
        Desktop::Gnome => gnome::steps(
            &payload_dir(options, session)?,
            &dirs,
            on_path(gnome::ENABLE_TOOL, session.path.as_deref()).as_deref(),
        )?,
        Desktop::Other => Vec::new(),
    };
    steps.extend(unless_packaged(
        dbus_activation::steps(&dirs, &exec),
        existing::packaged(&existing::service_candidates(
            &session.system,
            &dirs.home,
            dbus_activation::SERVICE_FILE,
        )),
        options.force,
        "D-Bus activation file",
    ));
    steps.extend(unless_packaged(
        systemd::steps(&dirs, &exec, systemctl.as_deref()),
        existing::packaged(&existing::unit_candidates(
            &session.system,
            &dirs.home,
            systemd::UNIT,
        )),
        options.force,
        "systemd user unit",
    ));
    steps.extend(desktop::shared_steps(&dirs, &exec));
    if target == Desktop::Other {
        steps.push(desktop::autostart_step(&dirs, &exec));
        steps.push(Step::Spawn {
            program: exec.clone(),
            args: vec!["tray".into()],
        });
    }
    if options.claude_statusline {
        let settings = claude_settings::settings_path(env, session.claude_config_dir.as_deref());
        steps.push(Step::Statusline(claude_settings::plan(&settings, &exec)?));
    }
    Ok(Plan {
        steps,
        desktop: target,
        exec,
    })
}

/// Drop `steps` when a package already provides `what` and `--force` was not given.
fn unless_packaged(
    steps: Vec<Step>,
    packaged: Option<PathBuf>,
    force: bool,
    what: &str,
) -> Vec<Step> {
    match packaged {
        Some(path) if !force => vec![Step::Note(format!(
            "a packaged {what} is already installed at {}, so it was left in charge \
             (re-run with --force to shadow it)",
            path.display()
        ))],
        _ => steps,
    }
}

/// Install everything, or print the plan when `--dry-run` was given.
pub fn setup(
    options: &SetupOptions,
    env: &Env,
    session: &Session,
    now: i64,
) -> anyhow::Result<String> {
    let plan = plan(options, env, session)?;
    let manifest_path = Manifest::path(env);
    let previous = Manifest::load(&manifest_path);
    if options.dry_run {
        return Ok(dry_run_summary(&plan, &manifest_path));
    }

    let owned = recorded_paths(previous.as_ref());
    let mut manifest = Manifest::new(&plan.exec, plan.desktop.as_str(), now);
    // The manifest is written after every step, so an interrupted run still
    // describes exactly what reached the disk.
    let outcome = match step::execute(
        &plan.steps,
        &mut manifest,
        Previous {
            owned: &owned,
            claude: previous.as_ref().and_then(|old| old.claude.as_ref()),
            force: options.force,
        },
        Some(&manifest_path),
    ) {
        Ok(outcome) => outcome,
        // Whatever did land is already recorded: store it, or `uninstall` would
        // have nothing to undo after a half-finished run.
        Err(error) => {
            return Err(save_partial(
                manifest,
                previous.as_ref(),
                &manifest_path,
                error,
            ));
        }
    };
    if outcome.succeeded(gnome::EXTENSION_UUID) {
        manifest.gnome_extension = Some(gnome::EXTENSION_UUID.to_string());
    }
    carry_forward(&mut manifest, previous.as_ref());
    manifest
        .save(&manifest_path)
        .with_context(|| format!("cannot write {}", manifest_path.display()))?;
    Ok(setup_summary(&plan, &manifest, &outcome, &manifest_path))
}

/// Every path a previous run recorded, so a re-run may rewrite exactly those.
fn recorded_paths(previous: Option<&Manifest>) -> Vec<PathBuf> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    previous
        .dirs
        .iter()
        .chain(previous.files.iter())
        .cloned()
        .collect()
}

/// Keep the record of a failed run, and say where it is.
fn save_partial(
    mut manifest: Manifest,
    previous: Option<&Manifest>,
    manifest_path: &Path,
    error: anyhow::Error,
) -> anyhow::Error {
    carry_forward(&mut manifest, previous);
    match manifest.save(manifest_path) {
        Ok(()) => error.context(format!(
            "setup did not finish; what it had installed is recorded in {} \
             and `token-station uninstall` will remove it",
            manifest_path.display()
        )),
        Err(write_error) => error.context(format!(
            "setup did not finish, and {} could not be written either: {write_error}",
            manifest_path.display()
        )),
    }
}

/// Keep what an earlier run installed so a later `uninstall` still finds it.
fn carry_forward(manifest: &mut Manifest, previous: Option<&Manifest>) {
    let Some(previous) = previous else { return };
    for dir in &previous.dirs {
        manifest.add_dir(dir);
    }
    for file in &previous.files {
        manifest.add_file(file);
    }
    // A unit an earlier run wrote and enabled is still enabled, even if this run
    // left a packaged copy in charge and wrote none of its own.
    if manifest.systemd_unit.is_none() {
        manifest.systemd_unit = previous.systemd_unit.clone();
    }
}

fn bullets(title: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let body: Vec<String> = lines.iter().map(|line| format!("  - {line}")).collect();
    format!("\n{title}\n{}", body.join("\n"))
}

fn dry_run_summary(plan: &Plan, manifest_path: &Path) -> String {
    let steps: Vec<String> = plan
        .steps
        .iter()
        .flat_map(Step::flattened)
        .map(Step::describe)
        .collect();
    format!(
        "Token Station setup (dry run: nothing was changed)\n  \
         binary:   {}\n  desktop:  {}\n  manifest: {}{}",
        plan.exec.display(),
        plan.desktop.as_str(),
        manifest_path.display(),
        bullets("Would:", &steps),
    )
}

fn setup_summary(
    plan: &Plan,
    manifest: &Manifest,
    outcome: &Outcome,
    manifest_path: &Path,
) -> String {
    let installed: Vec<String> = manifest
        .dirs
        .iter()
        .chain(manifest.files.iter())
        .map(|path| path.display().to_string())
        .collect();
    let statusline = manifest
        .claude
        .as_ref()
        .map(|record| vec![format!("{} · statusLine", record.settings.display())])
        .unwrap_or_default();
    format!(
        "Token Station is set up.\n  binary:   {}\n  desktop:  {}\n  manifest: {}{}{}{}{}",
        plan.exec.display(),
        plan.desktop.as_str(),
        manifest_path.display(),
        bullets("Installed:", &installed),
        bullets("Patched:", &statusline),
        bullets("Notes:", &outcome.notes),
        bullets("Warnings:", &outcome.warnings),
    )
}

/// Undo exactly what the manifest records.
pub fn uninstall(env: &Env, session: &Session) -> anyhow::Result<String> {
    let manifest_path = Manifest::path(env);
    let Some(manifest) = Manifest::load(&manifest_path) else {
        anyhow::bail!(
            "nothing to uninstall: no manifest at {}",
            manifest_path.display()
        );
    };
    let mut outcome = Outcome::default();
    let mut scratch = Manifest::new(&manifest.exec, &manifest.desktop, manifest.installed_at);
    let stopped = step::execute(
        &stop_steps(&manifest, session),
        &mut scratch,
        Previous::default(),
        None,
    )?;
    outcome.warnings.extend(stopped.warnings);
    outcome.notes.extend(stopped.notes);

    let claude = trusted_claude(&manifest, env, session, &mut outcome);
    let statusline = restore_statusline(claude, &mut outcome);
    let keep_backup = statusline.is_empty();
    let removed = remove_recorded(
        &manifest,
        claude,
        &Dirs::resolve(env),
        keep_backup,
        &mut outcome,
    );
    if let Err(error) = std::fs::remove_file(&manifest_path) {
        outcome
            .warnings
            .push(format!("cannot remove the manifest: {error}"));
    }
    Ok(format!(
        "Token Station is uninstalled.{}{}{}{}",
        bullets("Removed:", &removed),
        bullets("Restored:", &statusline),
        bullets("Notes:", &outcome.notes),
        bullets("Warnings:", &outcome.warnings),
    ))
}

/// Stop the tray and the unit, and disable the extension, before the files go.
fn stop_steps(manifest: &Manifest, session: &Session) -> Vec<Step> {
    let mut steps = vec![Step::StopTray {
        bus_address: session.bus_address.clone(),
    }];
    if manifest.systemd_unit.is_some() {
        steps.extend(systemd::disable_steps(
            on_path(systemd::SYSTEMCTL, session.path.as_deref()).as_deref(),
        ));
    }
    if let Some(uuid) = &manifest.gnome_extension {
        if let Some(tool) = on_path(gnome::ENABLE_TOOL, session.path.as_deref()) {
            steps.push(Step::Run {
                program: tool,
                args: vec!["disable".into(), uuid.clone()],
            });
        }
    }
    steps
}

/// The statusline record, if it describes the file `--claude-statusline` patches.
///
/// A manifest that names anything else is a manifest somebody edited: see
/// [`removal::trusted_claude_record`].
fn trusted_claude<'a>(
    manifest: &'a Manifest,
    env: &Env,
    session: &Session,
    outcome: &mut Outcome,
) -> Option<&'a ClaudeRecord> {
    let record = manifest.claude.as_ref()?;
    let settings = claude_settings::settings_path(env, session.claude_config_dir.as_deref());
    if removal::trusted_claude_record(record, &settings) {
        return Some(record);
    }
    outcome.warnings.push(format!(
        "the manifest's statusLine record names {} rather than {}; left both alone",
        record.settings.display(),
        settings.display()
    ));
    None
}

fn restore_statusline(claude: Option<&ClaudeRecord>, outcome: &mut Outcome) -> Vec<String> {
    let Some(record) = claude else {
        return Vec::new();
    };
    match claude_settings::restore(record) {
        Ok(true) => vec![format!("{} · statusLine", record.settings.display())],
        Ok(false) => {
            outcome.warnings.push(format!(
                "{} now has a different statusLine; left it alone",
                record.settings.display()
            ));
            Vec::new()
        }
        Err(error) => {
            outcome.warnings.push(format!("{error:#}"));
            Vec::new()
        }
    }
}

/// Remove what the manifest records, after checking each path is one `setup` writes.
///
/// `keep_backup` holds on to the `settings.json` copy when the status line was not
/// put back: it is the only way to recover the original by hand.
fn remove_recorded(
    manifest: &Manifest,
    claude: Option<&ClaudeRecord>,
    dirs: &Dirs,
    keep_backup: bool,
    outcome: &mut Outcome,
) -> Vec<String> {
    let backup = claude.map(|record| record.backup.clone());
    let mut removed = Vec::new();
    for dir in &manifest.dirs {
        if !removal::removable_dir(dir, dirs) {
            outcome.warnings.push(refused(dir));
            continue;
        }
        match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() => match std::fs::remove_dir_all(dir) {
                Ok(()) => removed.push(dir.display().to_string()),
                Err(error) => outcome
                    .warnings
                    .push(format!("cannot remove {}: {error}", dir.display())),
            },
            // Gone already, or replaced by something we do not own.
            _ => {}
        }
    }
    for file in &manifest.files {
        if keep_backup && backup.as_deref() == Some(file.as_path()) {
            outcome.notes.push(format!(
                "kept the backup at {}: the status line was not put back",
                file.display()
            ));
            continue;
        }
        if !removal::removable_file(file, dirs, backup.as_deref()) {
            outcome.warnings.push(refused(file));
            continue;
        }
        match std::fs::remove_file(file) {
            Ok(()) => removed.push(file.display().to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => outcome
                .warnings
                .push(format!("cannot remove {}: {error}", file.display())),
        }
    }
    removed
}

fn refused(path: &Path) -> String {
    format!(
        "{} is not a path Token Station installs; left it alone",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_for(root: &Path) -> Env {
        let at = |suffix: &str| Some(root.join(suffix).to_string_lossy().into_owned());
        Env {
            home: at("home"),
            config_home: at("config"),
            state_home: at("state"),
            cache_home: at("cache"),
            runtime_dir: at("run"),
            data_home: at("data"),
        }
    }

    #[test]
    fn the_roots_follow_the_xdg_variables() {
        let env = Env::from_map(&HashMap::from([
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/da"),
        ]));
        let dirs = Dirs::resolve(&env);
        assert_eq!(dirs.data, Path::new("/da"));
        assert_eq!(dirs.config, Path::new("/home/u/.config"));
        assert_eq!(dirs.state, Path::new("/home/u/.local/state"));
        assert_eq!(dirs.home, Path::new("/home/u"));
    }

    #[test]
    fn the_appimage_path_wins_over_the_current_binary() {
        let session = Session {
            appimage: Some("/apps/TokenStation.AppImage".into()),
            ..Session::default()
        };
        assert_eq!(
            exec_path(&session).unwrap(),
            Path::new("/apps/TokenStation.AppImage")
        );
        // Without it, the running binary is used, whatever the test runner is.
        assert_eq!(
            exec_path(&Session::default()).unwrap(),
            std::env::current_exe().unwrap()
        );
    }

    #[test]
    fn the_payload_comes_from_the_flag_or_from_appdir() {
        let explicit = SetupOptions {
            payload_dir: Some("/tmp/payload".into()),
            ..SetupOptions::default()
        };
        assert_eq!(
            payload_dir(&explicit, &Session::default()).unwrap(),
            Path::new("/tmp/payload")
        );

        let mounted = Session {
            appdir: Some("/tmp/.mount_abc".into()),
            ..Session::default()
        };
        assert_eq!(
            payload_dir(&SetupOptions::default(), &mounted).unwrap(),
            Path::new("/tmp/.mount_abc/usr/share/token-station/integrations")
        );

        let error = payload_dir(&SetupOptions::default(), &Session::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("--payload-dir"), "{error}");
        assert!(error.contains("$APPDIR"), "{error}");
    }

    #[test]
    fn desktop_choice_overrides_detection() {
        let session = Session {
            current_desktop: Some("KDE".into()),
            ..Session::default()
        };
        assert_eq!(DesktopChoice::Auto.resolve(&session), Desktop::Kde);
        assert_eq!(DesktopChoice::Gnome.resolve(&session), Desktop::Gnome);
        assert_eq!(DesktopChoice::Other.resolve(&session), Desktop::Other);
        assert_eq!(
            DesktopChoice::Auto.resolve(&Session::default()),
            Desktop::Other
        );
    }

    #[test]
    fn path_lookup_only_reads_path_and_only_accepts_executables() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("systemctl");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let path = dir.path().to_string_lossy().into_owned();

        assert_eq!(
            on_path("systemctl", Some(&path)),
            None,
            "not executable yet"
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(on_path("systemctl", Some(&path)), Some(tool));
        assert_eq!(on_path("systemctl", None), None);
        assert_eq!(on_path("nothing-here", Some(&path)), None);
    }

    #[test]
    fn a_dry_run_changes_nothing_and_prints_the_plan() {
        let root = tempfile::tempdir().unwrap();
        let env = env_for(root.path());
        let options = SetupOptions {
            desktop: DesktopChoice::Other,
            dry_run: true,
            ..SetupOptions::default()
        };
        let summary = setup(&options, &env, &Session::default(), 0).unwrap();

        assert!(summary.contains("dry run"), "{summary}");
        assert!(summary.contains("token-station.service"), "{summary}");
        assert!(!root.path().join("data").exists(), "nothing was written");
        assert!(!root.path().join("config").exists());
        assert!(!Manifest::path(&env).exists());
    }

    #[test]
    fn uninstalling_without_a_manifest_says_so() {
        let root = tempfile::tempdir().unwrap();
        let error = uninstall(&env_for(root.path()), &Session::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("nothing to uninstall"), "{error}");
    }
}
