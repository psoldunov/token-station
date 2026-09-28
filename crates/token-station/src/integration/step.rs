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
use crate::integration::manifest::{ClaudeRecord, Manifest};

/// A single thing `setup` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Replace `target` with a copy of the directory `source`; we own `target`.
    CopyTree { source: PathBuf, target: PathBuf },
    /// Copy one file into a directory shared with other applications.
    CopyFile { source: PathBuf, target: PathBuf },
    /// Write a generated file.
    Write { target: PathBuf, contents: String },
    /// Run a command and wait; a failure is a warning, not an error.
    Run { program: PathBuf, args: Vec<String> },
    /// Start a command and do not wait for it.
    Spawn { program: PathBuf, args: Vec<String> },
    /// Patch Claude Code's `settings.json`.
    Statusline(StatuslinePlan),
    /// A line for the summary; changes nothing.
    Note(String),
}

impl Step {
    /// One line describing what this step would do.
    pub fn describe(&self) -> String {
        match self {
            Step::CopyTree { source, target } => {
                format!("copy {} → {}", source.display(), target.display())
            }
            Step::CopyFile { source, target } => {
                format!("install {} → {}", source.display(), target.display())
            }
            Step::Write { target, .. } => format!("write {}", target.display()),
            Step::Run { program, args } => format!("run {}", command_line(program, args)),
            Step::Spawn { program, args } => format!("start {}", command_line(program, args)),
            Step::Statusline(plan) => format!(
                "patch {} · statusLine = {}",
                plan.settings.display(),
                plan.command
            ),
            Step::Note(text) => text.clone(),
        }
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
    pub fn succeeded(&self, program: &str) -> bool {
        self.ran
            .iter()
            .any(|(line, ok)| *ok && line.contains(program))
    }
}

/// Carry out every step, recording what stuck in `manifest`.
pub fn execute(
    steps: &[Step],
    manifest: &mut Manifest,
    recorded: Option<&ClaudeRecord>,
) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();
    for step in steps {
        match step {
            Step::CopyTree { source, target } => {
                copy_tree(source, target)?;
                manifest.add_dir(target);
            }
            Step::CopyFile { source, target } => {
                copy_file(source, target)?;
                manifest.add_file(target);
            }
            Step::Write { target, contents } => {
                write_atomic(target, contents.as_bytes())
                    .with_context(|| format!("cannot write {}", target.display()))?;
                manifest.add_file(target);
            }
            Step::Run { program, args } => run(program, args, &mut outcome),
            Step::Spawn { program, args } => spawn(program, args, &mut outcome),
            Step::Statusline(plan) => {
                let record = claude_settings::apply(plan, recorded)?;
                manifest.add_file(&record.backup);
                manifest.claude = Some(record);
            }
            Step::Note(text) => outcome.notes.push(text.clone()),
        }
    }
    Ok(outcome)
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

fn copy_into(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_into(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, target)
        .with_context(|| format!("cannot copy {} to {}", source.display(), target.display()))?;
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

    #[test]
    fn a_copied_tree_is_recorded_as_a_directory_we_own() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("payload/applet");
        std::fs::create_dir_all(source.join("contents/ui")).unwrap();
        std::fs::write(source.join("metadata.json"), "{}").unwrap();
        std::fs::write(source.join("contents/ui/main.qml"), "import").unwrap();
        let target = dir.path().join("data/plasmoids/dev.soldunov.tokenstation");

        let mut manifest = manifest();
        execute(
            &[Step::CopyTree {
                source: source.clone(),
                target: target.clone(),
            }],
            &mut manifest,
            None,
        )
        .unwrap();

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
        let source = dir.path().join("payload");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("new.qml"), "new").unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("stale.qml"), "old").unwrap();

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
        execute(
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
            None,
        )
        .unwrap();

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
        let outcome = execute(
            &[
                Step::Run {
                    program: PathBuf::from("/bin/false"),
                    args: vec!["--user".into()],
                },
                Step::Note("carried on".into()),
            ],
            &mut manifest,
            None,
        )
        .unwrap();
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("/bin/false --user"));
        assert_eq!(outcome.notes, vec!["carried on"]);
        assert!(!outcome.succeeded("/bin/false"));
    }

    #[test]
    fn a_command_that_does_not_exist_is_also_only_a_warning() {
        let mut manifest = manifest();
        let outcome = execute(
            &[Step::Run {
                program: PathBuf::from("/nonexistent/systemctl"),
                args: vec![],
            }],
            &mut manifest,
            None,
        )
        .unwrap();
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("could not start"));
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
}
