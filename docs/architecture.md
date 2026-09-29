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

### One daemon, thin front ends

Only the daemon reads credentials, calls the network or parses logs. The Plasma applet
and the GNOME extension just render the JSON snapshot it publishes. Business logic lives
in one place, and the GNOME extension never blocks the compositor. The applet is pure
QML with no compiled plugin tied to a Qt ABI, so the same package works from Nix and
from the AppImage.

### Shell-native front ends instead of a windowed tray app

Vanilla GNOME has no tray, and on Wayland a regular window can't be anchored next to a
tray icon. A Plasma tray applet and a Shell panel indicator get properly anchored popups,
and they follow the theme, accent colour and dark mode with no extra work.

### Read-only credentials

Both CLIs rotate their refresh tokens. If another program refreshed a token, it would
race the CLI and could sign the user out. So the Claude provider only reads the access
token. When that token has expired, it runs `claude auth status` and lets the CLI
refresh itself. The Codex provider leaves auth entirely to `codex app-server`.

### Codex child on demand

`codex app-server` takes about 150 MB of memory, and the first account request after
spawn takes about 6 s. The daemon spawns it for each refresh and stops it after
`linger_secs` of idleness. Set `process_mode = "persistent"` to keep it running.

### Local tokens and account tokens

`tokens` in the snapshot come from this machine's logs, with a per-bucket breakdown and
cost. Codex `accountTokens` come from the backend and cover every device. They measure
different things, so the front ends show them separately.

### Alerts fire once per window per reset period

Which alerts have fired is saved across restarts (`ts_core::alerts`). Some backends
jitter `resets_at` a little between responses, and the check tolerates that.

### Static musl AppImage

reqwest with rustls/ring, zbus (pure Rust) and a bundled SQLite mean the binary does not
depend on glibc, libdbus, OpenSSL or FUSE 2. The AppImage is a squashfs appended to the
pinned static type2 runtime, and it is assembled inside the Nix sandbox.

## Runtime files

- Config: `$XDG_CONFIG_HOME/token-station/config.toml`
- State: `$XDG_STATE_HOME/token-station/{history.sqlite,alerts.json,install-manifest.json}`
- Cache: `$XDG_CACHE_HOME/token-station/pricing.json`
- Runtime: `$XDG_RUNTIME_DIR/token-station/claude-statusline.json` (where the statusline
  collector drops its data when the daemon is not reachable)
