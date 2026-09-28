# Architecture

## Components

| Path | What it is |
|------|-----------|
| `crates/ts-core` | Domain model and contracts: the snapshot JSON (`snapshot.rs`, schema in `data/snapshot.schema.json`), config (`config.rs`), the `Provider` trait, token ledger, LiteLLM pricing, meter and alert-threshold logic. Pure, no IO beyond parsing. |
| `crates/ts-claude` | Claude Code provider: OAuth usage endpoint client, credentials reader (read-only), statusline ingest + merge, transcript scanner. |
| `crates/ts-codex` | Codex provider: `codex app-server` JSON-RPC client (on-demand child), HTTP fallback, rollout scanner. |
| `crates/token-station` | The binary: daemon (scheduler, D-Bus service, history store, notifications, pricing refresh, fixture mode), CLI commands, SNI tray, AppImage `setup`/`uninstall`. |
| `frontends/plasma/dev.soldunov.tokenstation` | Plasma 6 applet (pure QML) embedded in the system tray. |
| `frontends/gnome/token-station@soldunov.dev` | GNOME Shell extension (top-bar `PanelMenu.Button`) + libadwaita preferences. |
| `nix/` | Packages, home-manager/NixOS modules, AppImage assembly, GNOME VM test. |

## Design decisions

- **One daemon, thin front ends.** Only the daemon reads credentials, calls the network
  or parses logs. The Plasma applet and GNOME extension are renderers of one JSON
  snapshot, so business logic exists once, the GNOME extension never blocks the
  compositor, and the pure-QML applet has no compiled plugin tied to a Qt ABI (the same
  package works from Nix and from the AppImage).
- **Shell-native front ends instead of a windowed tray app.** Vanilla GNOME has no tray,
  and on Wayland a regular window cannot be anchored next to a tray icon. A Plasma tray
  applet and a Shell panel indicator get real anchored popups and inherit theme, accent
  and dark mode automatically.
- **Read-only credentials.** Both CLIs rotate refresh tokens; a third-party refresh would
  race the CLI and can sign the user out. The Claude provider only reads the access token
  and, when it has expired, asks `claude auth status` (the CLI refreshes itself). The
  Codex provider lets `codex app-server` handle auth entirely.
- **On-demand Codex child.** `codex app-server` holds ~150 MB; the first account request
  after spawn takes ~6 s. The daemon spawns it per refresh and stops it after
  `linger_secs` of idleness (`process_mode = "persistent"` keeps it).
- **Local vs account tokens.** `tokens` in the snapshot come from this machine's logs
  (with a per-bucket breakdown and cost); Codex `accountTokens` come from the backend and
  cover all devices. They are shown separately because they mean different things.
- **Alerts fire once per window per reset period**, persisted across restarts
  (`ts_core::alerts`), with a tolerance for backends jittering `resets_at`.
- **Static musl AppImage.** reqwest + rustls/ring, zbus (pure Rust), bundled SQLite: no
  glibc, libdbus, OpenSSL or FUSE 2 dependency. The AppImage is a squashfs appended to
  the pinned static type2 runtime, assembled inside the Nix sandbox.

## Runtime files

- Config: `$XDG_CONFIG_HOME/token-station/config.toml`
- State: `$XDG_STATE_HOME/token-station/{history.sqlite,alerts.json,install-manifest.json}`
- Cache: `$XDG_CACHE_HOME/token-station/pricing.json`
- Runtime: `$XDG_RUNTIME_DIR/token-station/claude-statusline.json` (statusline drop box
  used when the daemon is not reachable)
