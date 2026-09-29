//! The `PATH` a discovered CLI's child process gets.
//!
//! The case this exists for: a JavaScript package manager installs `codex` and
//! `claude` as scripts that defer to an interpreter found on `PATH` — the usual
//! shebang is `#!/usr/bin/env node`. Discovery finds the script, but the
//! daemon's own `PATH` is whatever launched it — on macOS, Finder's
//! `/usr/bin:/bin:/usr/sbin:/sbin` — and that has no `node` in it. Spawning the
//! script then fails looking for its interpreter.
//!
//! So these tests do not check a string: they run one.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use ts_core::discovery::{SearchEnv, child_path_in};

/// What a `PATH`-less launcher gives the daemon, and so the daemon's children.
const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Write `body` to `path` and make it executable.
fn write_executable(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("directory");
    }
    let mut file = std::fs::File::create(path).expect("created");
    file.write_all(body.as_bytes()).expect("written");
    drop(file);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// A home holding a fake interpreter and the scripts that need it.
struct Install {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
}

impl Install {
    /// Everything in `~/.local/bin`, one of the well-known directories.
    fn new() -> Install {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().to_path_buf();
        let bin = home.join(".local/bin");

        // Stands in for `node`: reachable only through the `PATH` the child is
        // given, never through the minimal one it would inherit.
        write_executable(
            &bin.join("fake-node"),
            "#!/bin/sh\necho ran-with-interpreter\n",
        );
        Install {
            _dir: dir,
            home,
            bin,
        }
    }

    /// A script that reaches its interpreter by name, as any such script does.
    ///
    /// `#!/bin/sh` rather than `#!/usr/bin/env fake-node`, because `/usr/bin`
    /// is not populated everywhere this runs — the nix build sandbox has
    /// `/bin/sh` and little else — and what is under test is the child's
    /// `PATH`, not the kernel's shebang handling. The literal `env` shebang is
    /// covered by [`the_env_shebang_form_works_the_same_way`] wherever
    /// `/usr/bin/env` exists.
    fn script(&self, name: &str) -> PathBuf {
        let path = self.bin.join(name);
        write_executable(&path, "#!/bin/sh\nexec fake-node\n");
        path
    }

    /// What the daemon would see when Finder or systemd started it.
    fn minimal_env(&self) -> SearchEnv {
        SearchEnv {
            path: Some(MINIMAL_PATH.into()),
            home: self.home.clone(),
            user: None,
        }
    }
}

/// Run `cli` with `path` as the child's whole `PATH`.
fn run(cli: &Path, path: &str) -> std::io::Result<std::process::Output> {
    Command::new(cli).env_clear().env("PATH", path).output()
}

/// Run `cli` with the `PATH` the daemon would build for it.
fn run_with_built_path(cli: &Path, env: &SearchEnv) -> std::io::Result<std::process::Output> {
    Command::new(cli)
        .env_clear()
        .env("PATH", child_path_in(cli, env))
        .output()
}

/// Did it run, and did the interpreter produce what only it can?
fn ran(output: &std::io::Result<std::process::Output>) -> bool {
    output.as_ref().is_ok_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == "ran-with-interpreter"
    })
}

#[test]
fn a_discovered_script_runs_with_the_path_its_interpreter_needs() {
    let install = Install::new();
    let cli = install.script("fake-cli");

    // With only what the launcher handed down, the interpreter is not there.
    assert!(
        !ran(&run(&cli, MINIMAL_PATH)),
        "the fake interpreter was reachable from the minimal PATH; \
         the test proves nothing"
    );

    // With the built one it is found next to the script, and the child runs.
    let output = run_with_built_path(&cli, &install.minimal_env());
    assert!(
        ran(&output),
        "{:?}",
        output.map(|output| (
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned()
        ))
    );
}

/// The same thing with the shebang a package-manager install really writes.
///
/// Skipped where `/usr/bin/env` is absent: the build sandbox has almost nothing
/// under `/usr`, and a missing `env` fails the `execve` outright, which says
/// nothing about the `PATH` the child was given.
#[test]
fn the_env_shebang_form_works_the_same_way() {
    if !Path::new("/usr/bin/env").exists() {
        eprintln!("skipping: no /usr/bin/env on this machine");
        return;
    }
    let install = Install::new();
    let cli = install.bin.join("fake-cli-env");
    write_executable(&cli, "#!/usr/bin/env fake-node\n");

    assert!(!ran(&run(&cli, MINIMAL_PATH)));
    assert!(ran(&run_with_built_path(&cli, &install.minimal_env())));
}

#[test]
fn the_built_path_still_starts_with_what_was_inherited() {
    let install = Install::new();
    let cli = install.script("fake-cli");
    let built = child_path_in(&cli, &install.minimal_env())
        .to_string_lossy()
        .into_owned();
    assert!(built.starts_with(&format!("{MINIMAL_PATH}:")), "{built}");
    // And the script's own directory comes next, which is what found the
    // interpreter above.
    let own = cli.parent().expect("parent").display().to_string();
    assert!(built.contains(&own), "{built}");
}
