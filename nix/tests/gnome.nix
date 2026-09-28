# Boots GNOME on Wayland, runs the daemon against a recorded fixture and
# checks that the Token Station extension loads, renders and opens its menu.
#
# Screenshots land in the test's output: `closed-light`, `open-light`,
# `closed-dark`, `open-dark`.
{
  pkgs,
  tokenStation,
  fixture,
}:
let
  uuid = "token-station@soldunov.dev";
  fixtureFile = pkgs.writeText "snapshot.json" (builtins.readFile fixture);
in
pkgs.testers.runNixOSTest {
  name = "token-station-gnome";

  nodes.machine =
    { lib, ... }:
    {
      virtualisation = {
        memorySize = 4096;
        cores = 2;
        # GNOME Shell needs a GPU it can fall back from; llvmpipe is enough.
        qemu.options = [ "-vga virtio" ];
      };

      users.users.alice = {
        isNormalUser = true;
        uid = 1000;
        description = "Alice";
      };

      services.displayManager.gdm.enable = true;
      services.displayManager.autoLogin = {
        enable = true;
        user = "alice";
      };
      services.desktopManager.gnome.enable = true;

      # Eval is internal API, so the Shell has to run unlocked for the test
      # driver to open the menu. Same trick as nixos/tests/gnome.nix.
      systemd.user.services."org.gnome.Shell@".serviceConfig.ExecStart = [
        ""
        "${pkgs.gnome-shell}/bin/gnome-shell --unsafe-mode"
      ];

      environment.systemPackages = [ tokenStation ];

      # Turn the extension on for the session, and keep the welcome dialog and
      # animations out of the screenshots.
      services.desktopManager.gnome.extraGSettingsOverrides = ''
        [org.gnome.shell]
        enabled-extensions=['${uuid}']
        disable-user-extensions=false
        welcome-dialog-last-shown-version='9999'

        [org.gnome.desktop.interface]
        enable-animations=false
      '';
      services.desktopManager.gnome.extraGSettingsOverridePackages = [
        pkgs.gnome-shell
        pkgs.gsettings-desktop-schemas
      ];

      # The packaged unit, pointed at the fixture instead of the real CLIs.
      systemd.user.services.token-station = {
        description = "Token Station usage daemon (fixture)";
        serviceConfig = {
          Type = "dbus";
          BusName = "dev.soldunov.TokenStation";
          ExecStart = lib.escapeShellArgs [
            "${tokenStation}/bin/token-station"
            "daemon"
            "--fixture"
            "${fixtureFile}"
          ];
          Restart = "on-failure";
          RestartSec = 2;
        };
        wantedBy = [ "default.target" ];
      };
    };

  testScript = ''
    import shlex

    UUID = "${uuid}"
    BUS = "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"


    def user(command):
        """Run a command inside alice's session bus."""
        return "su - alice -c " + shlex.quote(f"{BUS} {command}")


    def evaluate(js):
        """Call org.gnome.Shell.Eval; returns the raw gdbus tuple."""
        call = (
            "gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell "
            "-m org.gnome.Shell.Eval " + shlex.quote(js)
        )
        return machine.succeed(user(call))


    def set_color_scheme(scheme):
        machine.succeed(user(f"gsettings set org.gnome.desktop.interface color-scheme {scheme}"))
        # Give the Shell a moment to reload its stylesheet.
        machine.sleep(3)


    # The menu's label texts, joined with "|", so we can assert on content.
    MENU_TEXT_JS = """
    (() => {
      const indicator = Main.panel.statusArea[%r];
      if (!indicator) return 'NO-INDICATOR';
      const out = [];
      const walk = a => {
        if (typeof a.text === 'string' && a.text.length > 0) out.push(a.text);
        if (a.get_children) a.get_children().forEach(walk);
      };
      walk(indicator.menu.box);
      return out.join('|');
    })()
    """ % UUID


    with subtest("GNOME comes up"):
        machine.wait_for_unit("display-manager.service")
        machine.wait_for_file("/run/user/1000/wayland-0")
        machine.wait_for_unit("default.target", "alice")
        machine.wait_until_succeeds(
            user(
                "gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell "
                "-m org.gnome.Shell.Eval Main.layoutManager._startingUp"
            )
            + " | grep -q 'true,..false'"
        )
        # Leave the overview so the panel is on screen.
        machine.send_key("esc")

    with subtest("The daemon serves the fixture"):
        machine.wait_for_unit("token-station.service", "alice")
        machine.wait_until_succeeds(
            user(
                "busctl --user get-property dev.soldunov.TokenStation "
                "/dev/soldunov/TokenStation dev.soldunov.TokenStation1 Snapshot"
            )
            + " | grep -q schemaVersion"
        )

    with subtest("The extension is active and quiet"):
        machine.wait_until_succeeds(
            user(f"gnome-extensions info {UUID}") + " | grep -q 'State: ACTIVE'"
        )
        info = machine.succeed(user(f"gnome-extensions info {UUID}"))
        print(info)
        assert "Error" not in info, info
        # No JS errors attributed to the extension anywhere in the boot journal.
        machine.fail(
            "journalctl -b --no-pager | grep -E 'JS ERROR' | grep -q token-station"
        )
        # And the indicator really is in the panel.
        machine.succeed(
            user(
                "gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell "
                "-m org.gnome.Shell.Eval "
                + shlex.quote(f"!!Main.panel.statusArea[{UUID!r}]")
            )
            + " | grep -q 'true,..true'"
        )

    with subtest("It renders the fixture, light"):
        set_color_scheme("default")
        machine.sleep(5)
        machine.screenshot("closed-light")

        evaluate(f"Main.panel.statusArea[{UUID!r}].menu.open()")
        machine.sleep(3)
        text = evaluate(MENU_TEXT_JS)
        print(text)
        for expected in ["Claude Code", "Max 20x", "Codex", "Pro", "Session", "Weekly"]:
            assert expected in text, f"{expected!r} missing from the menu: {text}"
        # The countdown and the token summaries came from the fixture.
        # Percentages and counts use a narrow no-break space before the unit.
        assert "resets in" in text, text
        assert "96 %" in text, text
        assert "This device" in text, text
        assert "43.3 M" in text, text
        assert "All devices" in text, text
        machine.screenshot("open-light")

    with subtest("It renders the fixture, dark"):
        evaluate(f"Main.panel.statusArea[{UUID!r}].menu.close()")
        set_color_scheme("prefer-dark")
        machine.screenshot("closed-dark")
        evaluate(f"Main.panel.statusArea[{UUID!r}].menu.open()")
        machine.sleep(3)
        machine.screenshot("open-dark")
        evaluate(f"Main.panel.statusArea[{UUID!r}].menu.close()")

    with subtest("Preferences open against the running service"):
        set_color_scheme("default")
        machine.succeed(user(f"gnome-extensions prefs {UUID}"))
        machine.wait_until_succeeds(
            user(
                "gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell "
                "-m org.gnome.Shell.Eval "
                + shlex.quote(
                    "String(global.display.focus_window "
                    "&& global.display.focus_window.wm_class)"
                )
            )
            + " | grep -qi 'shell.extensions'"
        )
        machine.sleep(3)
        machine.screenshot("prefs")

    with subtest("Still no JS errors from the extension"):
        machine.fail(
            "journalctl -b --no-pager | grep -E 'JS ERROR' | grep -q token-station"
        )
  '';
}
