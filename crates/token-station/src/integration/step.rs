//! One installation plan, and the code that carries it out.
//!
//! `setup` builds a `Vec<Step>` first and only then touches the disk, so
//! `--dry-run` is the same plan printed instead of executed, and the manifest is
//! filled in from what actually succeeded.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context;

use crate::atomic::write_atomic;
use crate::integration::claude_settings::{self, Plan as StatuslinePlan};
use crate::integration::existing::{self, Previous, Verdict};
use crate::integration::manifest::Manifest;
use crate::tray;

/// A single thing `setup` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Replace `target` with a copy of the directory `source`; we own `target`.
    CopyTree { source: PathBuf, target: PathBuf },
    /// Copy one file into a directory shared with other applications.
    CopyFile { source: PathBuf, target: PathBuf },
    /// Write a generated file.
    Write { target: PathBuf, contents: String },
    /// Write the systemd user unit and, only if that write actually landed, carry
    /// out `then` (the `daemon-reload` and `enable --now` that load it).
    ///
    /// Nested rather than three steps in a row because the unit is the one file
    /// whose write decides whether anything else may happen: when a packaged or
    /// home-manager-linked unit is left in charge, `enable --now` would start a
    /// unit we do not own and `uninstall` would later `disable --now` it.
    Unit {
        target: PathBuf,
        contents: String,
        then: Vec<Step>,
    },
    /// Run a command and wait; a failure is a warning, not an error.
    Run { program: PathBuf, args: Vec<String> },
    /// Start a command and do not wait for it.
    Spawn { program: PathBuf, args: Vec<String> },
    /// Ask a running tray to quit, over its own bus name.
    StopTray {
        /// Which bus to look on; `None` means `$DBUS_SESSION_BUS_ADDRESS`.
        bus_address: Option<String>,
    },
    /// Patch Claude Code's `settings.json`.
    Statusline(StatuslinePlan),
    /// A line for the summary; changes nothing.
    Note(String),
}

impl Step {
    /// One line describing what this step would do.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Step::CopyTree { source, target } => {
                format!("copy {} → {}", source.display(), target.display())
            }
            Step::CopyFile { source, target } => {
                format!("install {} → {}", source.display(), target.display())
            }
            Step::Write { target, .. } | Step::Unit { target, .. } => {
                format!("write {}", target.display())
            }
            Step::Run { program, args } => format!("run {}", command_line(program, args)),
            Step::Spawn { program, args } => format!("start {}", command_line(program, args)),
            Step::StopTray { .. } => format!("stop the tray on {}", tray::TRAY_BUS_NAME),
            Step::Statusline(plan) => format!(
                "patch {} · statusLine = {}",
                plan.settings.display(),
                plan.command
            ),
            Step::Note(text) => text.clone(),
        }
    }

    /// Does this step create something the manifest has to record?
    fn writes_to_disk(&self) -> bool {
        matches!(
            self,
            Step::CopyTree { .. } | Step::CopyFile { .. } | Step::Write { .. } | Step::Unit { .. }
        )
    }

    /// This step and everything nested inside it, in the order they run.
    ///
    /// `--dry-run` prints the flattened list: the follow-up commands are part of
    /// what `setup` would do, and hiding them behind one line would understate it.
    pub fn flattened(&self) -> Vec<&Step> {
        let mut out = vec![self];
        if let Step::Unit { then, .. } = self {
            out.extend(then.iter().flat_map(Step::flattened));
        }
        out
    }
}

fn command_line(program: &Path, args: &[String]) -> String {
    std::iter::once(program.display().to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What executing a plan produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Commands that were run, and whether they succeeded.
    pub ran: Vec<(String, bool)>,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
}

impl Outcome {
    /// Did `program` run successfully?
    #[must_use]
    pub fn succeeded(&self, program: &str) -> bool {
        self.ran
            .iter()
            .any(|(line, ok)| *ok && line.contains(program))
    }
}

/// Carry out every step, recording what stuck in `manifest`.
///
/// A target that is already there and was not installed by us is skipped with a
/// warning rather than replaced; see [`existing`].
///
/// `journal` is where the manifest is written after every step that ran. Losing
/// power (or being killed) halfway through leaves a manifest describing exactly
/// what is on disk, which is what `uninstall` needs to undo it; `None` keeps
/// nothing, which is what `uninstall`'s own stop steps want.
///
/// # Errors
///
/// Returns the first step's error and stops there: a file or directory that
/// cannot be written, a tree that cannot be copied, or a command that cannot be
/// spawned or exits non-zero. The manifest still describes everything that did
/// land, so `uninstall` can undo a half-finished run.
pub fn execute(
    steps: &[Step],
    manifest: &mut Manifest,
    previous: Previous<'_>,
    journal: Option<&Path>,
) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();
    run_steps(steps, manifest, previous, journal, &mut outcome)?;
    Ok(outcome)
}

fn run_steps(
    steps: &[Step],
    manifest: &mut Manifest,
    previous: Previous<'_>,
    journal: Option<&Path>,
    outcome: &mut Outcome,
) -> anyhow::Result<()> {
    for step in steps {
        run_step(step, manifest, previous, journal, outcome)?;
        record(manifest, journal, outcome);
    }
    Ok(())
}

/// The steps that put something on disk, each recorded only once it landed.
///
/// Split from [`run_other_step`] because every one of these shares the same
/// shape — ask [`may_write`], write, record — and reading that shape once is
/// easier than reading it four times interleaved with the commands.
fn run_disk_step(
    step: &Step,
    manifest: &mut Manifest,
    previous: Previous<'_>,
    journal: Option<&Path>,
    outcome: &mut Outcome,
) -> anyhow::Result<()> {
    match step {
        Step::CopyTree { source, target } if may_write(target, previous, outcome) => {
            copy_tree(source, target)?;
            manifest.add_dir(target);
        }
        Step::CopyFile { source, target } if may_write(target, previous, outcome) => {
            copy_file(source, target)?;
            manifest.add_file(target);
        }
        Step::Write { target, contents } if may_write(target, previous, outcome) => {
            write(target, contents)?;
            manifest.add_file(target);
        }
        Step::Unit {
            target,
            contents,
            then,
        } if may_write(target, previous, outcome) => {
            write(target, contents)?;
            manifest.add_file(target);
            // Recorded only now: `uninstall` reads this to decide whether it may
            // stop and disable the unit, and it may only do that to a unit this
            // run wrote itself.
            manifest.systemd_unit = Some(target.clone());
            run_steps(then, manifest, previous, journal, outcome)?;
        }
        // Either refused (the warning is already recorded) or not a disk step.
        _ => {}
    }
    Ok(())
}

/// The steps that run something or say something.
fn run_other_step(
    step: &Step,
    manifest: &mut Manifest,
    previous: Previous<'_>,
    outcome: &mut Outcome,
) -> anyhow::Result<()> {
    match step {
        Step::Run { program, args } => run(program, args, outcome),
        Step::Spawn { program, args } => spawn(program, args, outcome),
        Step::StopTray { bus_address } => stop_tray(bus_address.as_deref(), outcome),
        Step::Statusline(plan) => {
            let record = claude_settings::apply(plan, previous.claude)?;
            manifest.add_file(&record.backup);
            manifest.claude = Some(record);
        }
        Step::Note(text) => outcome.notes.push(text.clone()),
        // Handled by `run_disk_step`.
        _ => {}
    }
    Ok(())
}

fn run_step(
    step: &Step,
    manifest: &mut Manifest,
    previous: Previous<'_>,
    journal: Option<&Path>,
    outcome: &mut Outcome,
) -> anyhow::Result<()> {
    if step.writes_to_disk() {
        return run_disk_step(step, manifest, previous, journal, outcome);
    }
    run_other_step(step, manifest, previous, outcome)
}

fn write(target: &Path, contents: &str) -> anyhow::Result<()> {
    write_atomic(target, contents.as_bytes())
        .with_context(|| format!("cannot write {}", target.display()))
}

/// Keep the on-disk manifest level with what has actually been installed.
fn record(manifest: &Manifest, journal: Option<&Path>, outcome: &mut Outcome) {
    let Some(path) = journal else { return };
    if let Err(error) = manifest.save(path) {
        let warning = format!("cannot update {}: {error}", path.display());
        if !outcome.warnings.contains(&warning) {
            outcome.warnings.push(warning);
        }
    }
}

/// May `target` be replaced? A refusal is recorded as a warning, not an error: the
/// rest of the install is still worth doing.
fn may_write(target: &Path, previous: Previous<'_>, outcome: &mut Outcome) -> bool {
    match existing::verdict(target, previous) {
        Verdict::Write => true,
        Verdict::Skip(why) => {
            outcome.warnings.push(why);
            false
        }
    }
}

/// Ask the tray to quit, so `uninstall` does not leave an icon behind.
fn stop_tray(bus_address: Option<&str>, outcome: &mut Outcome) {
    let line = format!("stop the tray on {}", tray::TRAY_BUS_NAME);
    match tray::client::request_quit_blocking(bus_address) {
        Ok(true) => outcome.ran.push((line, true)),
        Ok(false) => tracing::debug!("no tray was running"),
        Err(error) => {
            outcome
                .warnings
                .push(format!("cannot stop the tray: {error}"));
            outcome.ran.push((line, false));
        }
    }
}

/// Copy `source` over `target`, leaving nothing of a previous version behind.
fn copy_tree(source: &Path, target: &Path) -> anyhow::Result<()> {
    if std::fs::symlink_metadata(target).is_ok_and(|meta| meta.is_dir()) {
        std::fs::remove_dir_all(target)
            .with_context(|| format!("cannot replace {}", target.display()))?;
    }
    copy_into(source, target)
        .with_context(|| format!("cannot copy {} to {}", source.display(), target.display()))
}

/// Take a symlink out of the way, so what follows replaces the link itself.
///
/// [`existing::verdict`] has already decided we own this path, but a copy writes
/// *through* a link: `std::fs::copy` and `create_dir_all` both follow it, so
/// installing over a link the user made (with `--force`, or one an earlier run
/// recorded) would rewrite whatever it points at instead.
fn unlink_if_symlink(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(path),
        _ => Ok(()),
    }
}

fn copy_into(source: &Path, target: &Path) -> std::io::Result<()> {
    unlink_if_symlink(target)?;
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_into(&entry.path(), &to)?;
        } else {
            unlink_if_symlink(&to)?;
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let copy = unlink_if_symlink(target).and_then(|()| std::fs::copy(source, target));
    copy.with_context(|| format!("cannot copy {} to {}", source.display(), target.display()))?;
    Ok(())
}

fn run(program: &Path, args: &[String], outcome: &mut Outcome) {
    let line = command_line(program, args);
    let result = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output();
    let ok = match &result {
        Ok(output) => output.status.success(),
        Err(_) => false,
    };
    if !ok {
        outcome.warnings.push(match result {
            Ok(output) => format!(
                "`{line}` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => format!("`{line}` could not start: {error}"),
        });
    }
    outcome.ran.push((line, ok));
}

fn spawn(program: &Path, args: &[String], outcome: &mut Outcome) {
    let line = command_line(program, args);
    let started = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = &started {
        outcome
            .warnings
            .push(format!("`{line}` could not start: {error}"));
    }
    outcome.ran.push((line, started.is_ok()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::new(Path::new("/opt/x.AppImage"), "kde", 0)
    }

    /// Run `steps` on a fresh install with nothing recorded before it.
    fn fresh(steps: &[Step], manifest: &mut Manifest) -> Outcome {
        execute(steps, manifest, Previous::default(), None).expect("the plan runs")
    }

    /// `Previous` that recorded `owned` and nothing else.
    fn owning(owned: &[PathBuf]) -> Previous<'_> {
        Previous {
            owned,
            ..Previous::default()
        }
    }

    /// Create `base/sub` and write each `(relative_path, contents)` pair into it.
    fn dir_with_files(base: &Path, sub: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = base.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, contents) in files {
            let file = dir.join(name);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(file, contents).unwrap();
        }
        dir
    }

    #[test]
    fn a_copied_tree_is_recorded_as_a_directory_we_own() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("payload/applet");
        std::fs::create_dir_all(source.join("contents/ui")).unwrap();
        std::fs::write(source.join("metadata.json"), "{}").unwrap();
        std::fs::write(source.join("contents/ui/main.qml"), "import").unwrap();
        let target = dir.path().join("data/plasmoids/dev.soldunov.tokenstation");

        let mut manifest = manifest();
        fresh(
            &[Step::CopyTree {
                source: source.clone(),
                target: target.clone(),
            }],
            &mut manifest,
        );

        assert_eq!(
            std::fs::read_to_string(target.join("contents/ui/main.qml")).unwrap(),
            "import"
        );
        assert_eq!(manifest.dirs, vec![target.clone()]);
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn copying_again_removes_files_the_old_version_left() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir_with_files(dir.path(), "payload", &[("new.qml", "new")]);
        let target = dir_with_files(dir.path(), "target", &[("stale.qml", "old")]);

        copy_tree(&source, &target).unwrap();
        assert!(target.join("new.qml").exists());
        assert!(!target.join("stale.qml").exists());
    }

    #[test]
    fn written_and_copied_files_are_recorded_one_by_one() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("icon.svg");
        std::fs::write(&source, "<svg/>").unwrap();
        let icon = dir.path().join("data/icons/hicolor/scalable/apps/icon.svg");
        let unit = dir.path().join("cfg/systemd/user/token-station.service");

        let mut manifest = manifest();
        fresh(
            &[
                Step::CopyFile {
                    source,
                    target: icon.clone(),
                },
                Step::Write {
                    target: unit.clone(),
                    contents: "[Unit]\n".into(),
                },
            ],
            &mut manifest,
        );

        assert_eq!(std::fs::read_to_string(&icon).unwrap(), "<svg/>");
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), "[Unit]\n");
        assert_eq!(manifest.files, vec![icon, unit]);
        assert!(
            manifest.dirs.is_empty(),
            "shared directories stay untouched"
        );
    }

    #[test]
    fn a_failing_command_is_a_warning_and_the_plan_carries_on() {
        let mut manifest = manifest();
        let outcome = fresh(
            &[
                Step::Run {
                    program: PathBuf::from("/bin/false"),
                    args: vec!["--user".into()],
                },
                Step::Note("carried on".into()),
            ],
            &mut manifest,
        );
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("/bin/false --user"));
        assert_eq!(outcome.notes, vec!["carried on"]);
        assert!(!outcome.succeeded("/bin/false"));
    }

    #[test]
    fn a_command_that_does_not_exist_is_also_only_a_warning() {
        let mut manifest = manifest();
        let outcome = fresh(
            &[Step::Run {
                program: PathBuf::from("/nonexistent/systemctl"),
                args: vec![],
            }],
            &mut manifest,
        );
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("could not start"));
    }

    #[test]
    fn a_target_we_did_not_install_is_skipped_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dev.soldunov.TokenStation.service");
        std::fs::write(&target, "someone else's file").unwrap();

        let mut manifest = manifest();
        let outcome = fresh(
            &[Step::Write {
                target: target.clone(),
                contents: "[D-BUS Service]\n".into(),
            }],
            &mut manifest,
        );

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "someone else's file"
        );
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("--force"), "{outcome:?}");
        assert!(
            manifest.files.is_empty(),
            "a file we skipped is not recorded as ours"
        );
    }

    #[test]
    fn a_target_an_earlier_run_installed_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("token-station.service");
        std::fs::write(&target, "[Unit]\n# old\n").unwrap();
        let owned = vec![target.clone()];

        let mut manifest = manifest();
        let outcome = execute(
            &[Step::Write {
                target: target.clone(),
                contents: "[Unit]\n# new\n".into(),
            }],
            &mut manifest,
            owning(&owned),
            None,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "[Unit]\n# new\n");
        assert!(outcome.warnings.is_empty(), "{outcome:?}");
        assert_eq!(manifest.files, vec![target]);
    }

    #[test]
    fn a_store_symlink_in_the_way_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("token-station.service");
        std::os::unix::fs::symlink("/nix/store/abc-token-station/unit", &target).unwrap();
        let owned = vec![target.clone()];

        let mut manifest = manifest();
        let outcome = execute(
            &[Step::Write {
                target: target.clone(),
                contents: "[Unit]\n".into(),
            }],
            &mut manifest,
            Previous {
                owned: &owned,
                force: true,
                ..Previous::default()
            },
            None,
        )
        .unwrap();

        assert!(std::fs::symlink_metadata(&target).unwrap().is_symlink());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("/nix/store"), "{outcome:?}");
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn a_copied_tree_over_a_stranger_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir_with_files(dir.path(), "payload", &[("metadata.json", "{}")]);
        let target = dir_with_files(
            dir.path(),
            "plasmoids/dev.soldunov.tokenstation",
            &[("theirs.qml", "mine")],
        );

        let mut manifest = manifest();
        let outcome = fresh(
            &[Step::CopyTree {
                source,
                target: target.clone(),
            }],
            &mut manifest,
        );

        assert!(target.join("theirs.qml").exists(), "nothing was removed");
        assert!(!target.join("metadata.json").exists());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(manifest.dirs.is_empty());
    }

    /// The unit step as `setup` builds it: a write plus the two systemctl calls.
    fn unit_step(target: &Path, tool: &Path) -> Step {
        Step::Unit {
            target: target.to_path_buf(),
            contents: "[Unit]\n".into(),
            then: vec![Step::Run {
                program: tool.to_path_buf(),
                args: vec!["--user".into(), "daemon-reload".into()],
            }],
        }
    }

    #[test]
    fn the_unit_is_recorded_and_loaded_only_when_its_write_landed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("token-station.service");

        let mut manifest = manifest();
        let tool = dir.path().join("systemctl");
        let outcome = fresh(&[unit_step(&target, &tool)], &mut manifest);

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "[Unit]\n");
        assert_eq!(manifest.systemd_unit, Some(target.clone()));
        assert_eq!(manifest.files, vec![target]);
        assert_eq!(outcome.ran.len(), 1, "systemctl was called: {outcome:?}");
    }

    #[test]
    fn a_unit_somebody_else_owns_is_neither_recorded_nor_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("token-station.service");
        std::os::unix::fs::symlink("/nix/store/abc-token-station/unit", &target).unwrap();

        let mut manifest = manifest();
        let outcome = fresh(
            &[unit_step(&target, &dir.path().join("systemctl"))],
            &mut manifest,
        );

        assert_eq!(manifest.systemd_unit, None, "we do not own it");
        assert!(manifest.files.is_empty());
        assert!(
            outcome.ran.is_empty(),
            "systemctl must not run: {outcome:?}"
        );
        assert_eq!(outcome.warnings.len(), 1);
    }

    #[test]
    fn the_manifest_is_written_after_every_step_that_ran() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("state/install-manifest.json");
        let first = dir.path().join("first.service");
        let second = dir.path().join("second.desktop");

        let mut manifest = manifest();
        execute(
            &[
                Step::Write {
                    target: first.clone(),
                    contents: "a".into(),
                },
                // Reading the journal from inside the run is the only way to see
                // the intermediate state, so the second step does the reading.
                Step::Write {
                    target: second.clone(),
                    contents: "b".into(),
                },
            ],
            &mut manifest,
            Previous::default(),
            Some(&journal),
        )
        .unwrap();

        let written = Manifest::load(&journal).expect("a manifest was journalled");
        assert_eq!(written.files, vec![first, second]);
    }

    #[test]
    fn a_copy_replaces_a_symlink_instead_of_writing_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("icon.svg");
        std::fs::write(&source, "<svg/>").unwrap();
        // What the user's dotfiles put there, and what must survive untouched.
        let elsewhere = dir.path().join("their-icon.svg");
        std::fs::write(&elsewhere, "theirs").unwrap();
        let target = dir.path().join("data/icons/icon.svg");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
        let owned = vec![target.clone()];

        let mut manifest = manifest();
        execute(
            &[Step::CopyFile {
                source,
                target: target.clone(),
            }],
            &mut manifest,
            owning(&owned),
            None,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "theirs");
        assert!(!std::fs::symlink_metadata(&target).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "<svg/>");
    }

    #[test]
    fn a_copied_tree_replaces_a_symlinked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir_with_files(dir.path(), "payload", &[("metadata.json", "{}")]);
        let elsewhere = dir_with_files(dir.path(), "theirs", &[("keep.qml", "theirs")]);
        let target = dir.path().join("data/plasmoids/dev.soldunov.tokenstation");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
        let owned = vec![target.clone()];

        let mut manifest = manifest();
        execute(
            &[Step::CopyTree {
                source,
                target: target.clone(),
            }],
            &mut manifest,
            owning(&owned),
            None,
        )
        .unwrap();

        assert!(elsewhere.join("keep.qml").exists(), "their tree survives");
        assert!(!elsewhere.join("metadata.json").exists());
        assert!(!std::fs::symlink_metadata(&target).unwrap().is_symlink());
        assert!(target.join("metadata.json").exists());
    }

    #[test]
    fn every_step_describes_itself_for_the_dry_run() {
        let described = Step::CopyTree {
            source: "/a".into(),
            target: "/b".into(),
        }
        .describe();
        assert_eq!(described, "copy /a → /b");
        assert_eq!(
            Step::Run {
                program: "/bin/systemctl".into(),
                args: vec!["--user".into(), "daemon-reload".into()]
            }
            .describe(),
            "run /bin/systemctl --user daemon-reload"
        );
        assert_eq!(Step::Note("hello".into()).describe(), "hello");
    }

    #[test]
    fn a_nested_unit_step_flattens_into_the_lines_it_would_run() {
        let step = unit_step(
            Path::new("/cfg/token-station.service"),
            Path::new("/bin/sc"),
        );
        let lines: Vec<String> = step.flattened().iter().map(|s| s.describe()).collect();
        assert_eq!(
            lines,
            vec![
                "write /cfg/token-station.service",
                "run /bin/sc --user daemon-reload",
            ]
        );
        assert_eq!(Step::Note("x".into()).flattened().len(), 1);
    }
}
