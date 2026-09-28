//! `setup` and `uninstall` round trips in a throwaway home.
//!
//! Every run works on temp XDG directories and a `PATH` holding only recording
//! stand-ins for `systemctl` and `gnome-extensions`, so nothing here touches the
//! real session.

use std::path::{Path, PathBuf};

use token_station::integration::desktop::Desktop;
use token_station::integration::manifest::Manifest;
use token_station::integration::{self, DesktopChoice, Session, SetupOptions};
use token_station::paths::Env;

const NOW: i64 = 1_790_596_800;
const EXEC: &str = "/apps/TokenStation-x86_64.AppImage";

/// A temp home with a fake `PATH` and a front-end payload.
struct Home {
    dir: tempfile::TempDir,
    log: PathBuf,
    bin: PathBuf,
}

impl Home {
    fn new(tools: &[&str]) -> Home {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("invocations.log");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in tools {
            write_recorder(&bin.join(tool), tool, &log);
        }
        Home { dir, log, bin }
    }

    fn env(&self) -> Env {
        let at = |suffix: &str| Some(self.dir.path().join(suffix).to_string_lossy().into_owned());
        Env {
            home: at("home"),
            config_home: at("config"),
            state_home: at("state"),
            cache_home: at("cache"),
            runtime_dir: at("run"),
            data_home: at("data"),
        }
    }

    fn session(&self, desktop: Option<&str>) -> Session {
        Session {
            current_desktop: desktop.map(str::to_string),
            appimage: Some(EXEC.into()),
            appdir: None,
            path: Some(self.bin.to_string_lossy().into_owned()),
            claude_config_dir: None,
        }
    }

    /// The `integrations` payload an AppImage would carry.
    fn payload(&self) -> PathBuf {
        let payload = self.dir.path().join("integrations");
        for (front_end, file) in [("plasma", "metadata.json"), ("gnome", "metadata.json")] {
            let sub = payload.join(front_end).join("contents");
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("marker"), front_end).unwrap();
            std::fs::write(payload.join(front_end).join(file), "{}").unwrap();
        }
        payload
    }

    fn path(&self, suffix: &str) -> PathBuf {
        self.dir.path().join(suffix)
    }

    /// Everything the fake tools were asked to do, one command per line.
    fn invocations(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn write_recorder(path: &Path, name: &str, log: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!("#!/bin/sh\necho \"{name} $*\" >> '{}'\n", log.display());
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn options(desktop: DesktopChoice, payload: Option<PathBuf>) -> SetupOptions {
    SetupOptions {
        desktop,
        claude_statusline: false,
        payload_dir: payload,
        dry_run: false,
    }
}

fn manifest_of(home: &Home) -> Manifest {
    Manifest::load(&Manifest::path(&home.env())).expect("a manifest was written")
}

#[test]
fn a_kde_setup_installs_the_applet_the_unit_and_the_activation_file() {
    let home = Home::new(&["systemctl"]);
    let payload = home.payload();
    let summary = integration::setup(
        &options(DesktopChoice::Auto, Some(payload)),
        &home.env(),
        &home.session(Some("KDE")),
        NOW,
    )
    .expect("setup succeeds");

    assert!(summary.contains("Token Station is set up"), "{summary}");
    assert!(summary.contains("desktop:  kde"), "{summary}");

    let applet = home.path("data/plasma/plasmoids/dev.soldunov.tokenstation");
    assert_eq!(
        std::fs::read_to_string(applet.join("contents/marker")).unwrap(),
        "plasma"
    );
    let unit = home.path("config/systemd/user/token-station.service");
    assert!(
        std::fs::read_to_string(&unit)
            .unwrap()
            .contains(&format!("ExecStart=\"{EXEC}\" daemon"))
    );
    let service = home.path("data/dbus-1/services/dev.soldunov.TokenStation.service");
    let service_text = std::fs::read_to_string(&service).unwrap();
    assert!(service_text.contains(&format!("Exec=\"{EXEC}\" daemon")));
    assert!(service_text.contains("SystemdService=token-station.service"));
    assert!(
        home.path("data/applications/dev.soldunov.TokenStation.desktop")
            .exists()
    );
    assert!(
        home.path("data/icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg")
            .exists()
    );
    // KDE has its own applet: no tray autostart.
    assert!(!home.path("config/autostart").exists());

    assert_eq!(
        home.invocations(),
        vec![
            "systemctl --user daemon-reload",
            "systemctl --user enable --now token-station.service",
        ]
    );

    let manifest = manifest_of(&home);
    assert_eq!(manifest.desktop, Desktop::Kde.as_str());
    assert_eq!(manifest.exec, Path::new(EXEC));
    assert_eq!(manifest.dirs, vec![applet]);
    assert_eq!(manifest.systemd_unit, Some(unit));
    assert_eq!(manifest.gnome_extension, None);
    assert!(manifest.files.contains(&service));
}

#[test]
fn a_gnome_setup_enables_the_extension_and_warns_about_wayland() {
    let home = Home::new(&["systemctl", "gnome-extensions"]);
    let payload = home.payload();
    let summary = integration::setup(
        &options(DesktopChoice::Auto, Some(payload)),
        &home.env(),
        &home.session(Some("ubuntu:GNOME")),
        NOW,
    )
    .unwrap();

    assert!(summary.contains("desktop:  gnome"), "{summary}");
    assert!(summary.contains("log out and back in"), "{summary}");
    let extension = home.path("data/gnome-shell/extensions/token-station@soldunov.dev");
    assert_eq!(
        std::fs::read_to_string(extension.join("contents/marker")).unwrap(),
        "gnome"
    );
    assert!(
        home.invocations()
            .contains(&"gnome-extensions enable token-station@soldunov.dev".to_string())
    );

    let manifest = manifest_of(&home);
    assert_eq!(
        manifest.gnome_extension.as_deref(),
        Some("token-station@soldunov.dev")
    );
    assert_eq!(manifest.dirs, vec![extension]);
}

#[test]
fn every_other_desktop_gets_the_tray_autostart() {
    let home = Home::new(&["systemctl"]);
    // No payload is needed when neither native front end is installed.
    integration::setup(
        &options(DesktopChoice::Auto, None),
        &home.env(),
        &home.session(Some("sway")),
        NOW,
    )
    .unwrap();

    let autostart = home.path("config/autostart/dev.soldunov.TokenStation.Tray.desktop");
    let text = std::fs::read_to_string(&autostart).unwrap();
    assert!(text.contains(&format!("Exec=\"{EXEC}\" tray")), "{text}");
    let manifest = manifest_of(&home);
    assert_eq!(manifest.desktop, "other");
    assert!(manifest.files.contains(&autostart));
    assert!(manifest.dirs.is_empty());
}

#[test]
fn uninstall_removes_exactly_what_the_manifest_records() {
    let home = Home::new(&["systemctl", "gnome-extensions"]);
    let payload = home.payload();
    integration::setup(
        &options(DesktopChoice::Gnome, Some(payload)),
        &home.env(),
        &home.session(Some("GNOME")),
        NOW,
    )
    .unwrap();

    // A file we did not install, in a directory we wrote into.
    let stranger = home.path("data/applications/someone-else.desktop");
    std::fs::write(&stranger, "[Desktop Entry]").unwrap();

    let summary = integration::uninstall(&home.env(), &home.session(Some("GNOME"))).unwrap();
    assert!(
        summary.contains("Token Station is uninstalled"),
        "{summary}"
    );

    assert!(
        !home
            .path("data/gnome-shell/extensions/token-station@soldunov.dev")
            .exists()
    );
    assert!(
        !home
            .path("config/systemd/user/token-station.service")
            .exists()
    );
    assert!(
        !home
            .path("data/dbus-1/services/dev.soldunov.TokenStation.service")
            .exists()
    );
    assert!(
        !home
            .path("data/applications/dev.soldunov.TokenStation.desktop")
            .exists()
    );
    assert!(!Manifest::path(&home.env()).exists());
    assert!(stranger.exists(), "a file we never installed stays");

    let calls = home.invocations();
    assert!(calls.contains(&"systemctl --user disable --now token-station.service".to_string()));
    assert!(calls.contains(&"gnome-extensions disable token-station@soldunov.dev".to_string()));
}

#[test]
fn rerunning_setup_updates_the_paths_and_keeps_one_manifest() {
    let home = Home::new(&["systemctl"]);
    let session = home.session(Some("sway"));
    integration::setup(
        &options(DesktopChoice::Auto, None),
        &home.env(),
        &session,
        NOW,
    )
    .unwrap();

    let moved = Session {
        appimage: Some("/opt/TokenStation.AppImage".into()),
        ..session
    };
    integration::setup(
        &options(DesktopChoice::Auto, None),
        &home.env(),
        &moved,
        NOW + 60,
    )
    .unwrap();

    let unit =
        std::fs::read_to_string(home.path("config/systemd/user/token-station.service")).unwrap();
    assert!(
        unit.contains("ExecStart=\"/opt/TokenStation.AppImage\" daemon"),
        "{unit}"
    );
    let manifest = manifest_of(&home);
    assert_eq!(manifest.exec, Path::new("/opt/TokenStation.AppImage"));
    assert_eq!(manifest.installed_at, NOW + 60);
    // The file list did not grow: the same paths were rewritten.
    let unique: std::collections::BTreeSet<_> = manifest.files.iter().collect();
    assert_eq!(unique.len(), manifest.files.len());
}

#[test]
fn switching_desktops_still_uninstalls_the_first_front_end() {
    let home = Home::new(&["systemctl"]);
    let payload = home.payload();
    integration::setup(
        &options(DesktopChoice::Kde, Some(payload.clone())),
        &home.env(),
        &home.session(Some("KDE")),
        NOW,
    )
    .unwrap();
    integration::setup(
        &options(DesktopChoice::Gnome, Some(payload)),
        &home.env(),
        &home.session(Some("GNOME")),
        NOW,
    )
    .unwrap();

    let manifest = manifest_of(&home);
    assert_eq!(manifest.dirs.len(), 2, "{:?}", manifest.dirs);
    integration::uninstall(&home.env(), &home.session(Some("GNOME"))).unwrap();
    assert!(
        !home
            .path("data/plasma/plasmoids/dev.soldunov.tokenstation")
            .exists()
    );
    assert!(
        !home
            .path("data/gnome-shell/extensions/token-station@soldunov.dev")
            .exists()
    );
}

#[test]
fn the_claude_status_line_is_wrapped_backed_up_and_put_back() {
    let home = Home::new(&["systemctl"]);
    let claude = home.path("home/.claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(
        &settings,
        r#"{"model": "opus", "statusLine": {"type": "command", "command": "starship"}}"#,
    )
    .unwrap();

    let mut options = options(DesktopChoice::Other, None);
    options.claude_statusline = true;
    let summary = integration::setup(&options, &home.env(), &home.session(None), NOW).unwrap();
    assert!(summary.contains("statusLine"), "{summary}");

    let patched: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        patched["statusLine"]["command"],
        format!("'{EXEC}' statusline --wrap 'starship'")
    );
    assert_eq!(patched["model"], "opus");
    assert!(claude.join("settings.json.token-station-backup").exists());

    // Running it again does not nest a second wrapper.
    integration::setup(&options, &home.env(), &home.session(None), NOW).unwrap();
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(again["statusLine"], patched["statusLine"]);

    integration::uninstall(&home.env(), &home.session(None)).unwrap();
    let restored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], "starship");
    assert!(!claude.join("settings.json.token-station-backup").exists());
}

#[test]
fn a_symlinked_claude_settings_file_stops_setup_before_anything_is_written() {
    let home = Home::new(&["systemctl"]);
    let claude = home.path("home/.claude");
    std::fs::create_dir_all(&claude).unwrap();
    let real = home.path("home/store-settings.json");
    std::fs::write(&real, "{}").unwrap();
    std::os::unix::fs::symlink(&real, claude.join("settings.json")).unwrap();

    let mut options = options(DesktopChoice::Other, None);
    options.claude_statusline = true;
    let error = integration::setup(&options, &home.env(), &home.session(None), NOW)
        .unwrap_err()
        .to_string();

    assert!(error.contains("symlink"), "{error}");
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "{}");
    // The plan is built before anything runs, so nothing was installed either.
    assert!(!home.path("data").exists());
    assert!(!home.path("config").exists());
    assert!(home.invocations().is_empty());
}

#[test]
fn a_dry_run_lists_every_step_and_changes_nothing() {
    let home = Home::new(&["systemctl", "gnome-extensions"]);
    let payload = home.payload();
    let mut options = options(DesktopChoice::Gnome, Some(payload));
    options.dry_run = true;
    let summary =
        integration::setup(&options, &home.env(), &home.session(Some("GNOME")), NOW).unwrap();

    assert!(summary.contains("dry run"), "{summary}");
    for expected in [
        "gnome-shell/extensions/token-station@soldunov.dev",
        "systemd/user/token-station.service",
        "dbus-1/services/dev.soldunov.TokenStation.service",
        "enable --now",
    ] {
        assert!(
            summary.contains(expected),
            "{expected} missing from:\n{summary}"
        );
    }
    assert!(!home.path("data").exists());
    assert!(!home.path("config").exists());
    assert!(!Manifest::path(&home.env()).exists());
    assert!(home.invocations().is_empty());
}
