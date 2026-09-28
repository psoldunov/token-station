# D-Bus API

The daemon (`token-station daemon`) owns the session-bus name `dev.soldunov.TokenStation`
and exports one object. Front ends (Plasma applet, GNOME extension, SNI tray, CLI) only
talk to this interface.

- Bus name: `dev.soldunov.TokenStation`
- Object path: `/dev/soldunov/TokenStation`
- Interface: `dev.soldunov.TokenStation1`

The bus name is D-Bus activatable (`share/dbus-1/services/dev.soldunov.TokenStation.service`),
so calling any method starts the daemon if it is not running.

## Properties

| Name       | Type | Access | Notes |
|------------|------|--------|-------|
| `Snapshot` | `s`  | read   | Full state as JSON (see below). Emits `org.freedesktop.DBus.Properties.PropertiesChanged` with the new value whenever it changes. |
| `Revision` | `t`  | read   | Increments with every new snapshot. Changes together with `Snapshot`. |

## Methods

| Signature | Description |
|-----------|-------------|
| `Refresh() → ()` | Refresh every provider now. Coalesced: concurrent calls share one refresh. Providers still apply their hard rate-limit backoff (forced endpoint calls are at least 30 s apart). Returns once the refresh finished. |
| `GetHistory(s provider, s window_id, t since) → s` | JSON array `[[ts, percent], …]` of recorded samples for one window since `since` (Unix seconds), oldest first, down-sampled to at most 120 points. Unknown provider/window returns `[]`. |
| `GetSettings() → s` | Current config as JSON (snake_case keys, same structure as `config.toml`, see `crates/ts-core/src/config.rs`). |
| `SetSettings(s json) → ()` | Replace the config. Missing keys take defaults. Validated (bounds, unknown keys rejected) and written to `config.toml`; invalid input fails with `org.freedesktop.DBus.Error.InvalidArgs` and a message listing every problem. Applies immediately. |
| `IngestClaudeStatusline(s json) → ()` | Used by `token-station statusline`. Raw statusline JSON from Claude Code (max 64 KiB). Invalid input fails with `InvalidArgs`. |

## Snapshot JSON

camelCase keys, Unix-second timestamps, `schemaVersion` = 1. The authoritative schema is
`data/snapshot.schema.json` (generated from `crates/ts-core/src/snapshot.rs`; a test fails
when it is stale). Example scenarios live in `data/fixtures/snapshot-*.json`.

```jsonc
{
  "schemaVersion": 1,
  "revision": 42,
  "generatedAt": 1790596800,
  "meter": {
    // one bar per enabled provider, Claude first; percent null = draw an empty outline
    "bars": [
      { "provider": "claude", "percent": 34.0, "level": "normal", "windowId": "session" },
      { "provider": "codex", "percent": 12.0, "level": "normal", "windowId": "codex:primary" }
    ],
    "level": "normal"                         // worst bar level: normal | warning | critical
  },
  "providers": [{
    "id": "claude",                           // claude | codex
    "name": "Claude Code",
    "state": "ok",                            // loading | ok | stale | unauthenticated | not_installed | disabled | error
    "message": null,                          // user-facing text for non-ok states
    "plan": "Max 20x",
    "windows": [{
      "id": "session", "label": "Session", "kind": "session",   // kind: session | weekly | model | other
      "usedPercent": 34.0, "resetsAt": 1790614800, "windowMinutes": 300,
      "level": "normal", "source": "oauth", "observedAt": 1790596740
    }],
    "credits": { "label": "Extra usage", "enabled": false, "used": 0.0, "limit": null,
                 "currency": "USD", "percent": null, "detail": "Turned off" },
    "breakdown": [ { "key": "claude_code", "label": "Claude Code", "percent": 97.0 } ],
    "tokens": {                               // this machine's local logs; null if none
      "today":     { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "reasoning": 0,
                     "total": 0, "costUsd": 0.0, "unpricedTokens": 0, "requests": 0 },
      "last7Days": { "...": "same shape" },
      "byModel":   [ { "model": "claude-opus-5-5", "total": 0, "costUsd": 0.0, "...": "same shape" } ],
      "updatedAt": 1790596770
    },
    "accountTokens": {                        // backend-reported, all devices (Codex); null if unavailable
      "today": 0, "last7Days": 0, "lifetime": 0,
      "daily": [ { "date": "2026-09-28", "tokens": 0 } ], "updatedAt": 1790596740
    },
    "updatedAt": 1790596740                   // last successful plan-limit refresh
  }]
}
```

Front ends compute "resets in …" themselves from `resetsAt` so countdowns stay live
between snapshots. `level` values are already classified against the user's thresholds.

## Examples

```sh
busctl --user introspect dev.soldunov.TokenStation /dev/soldunov/TokenStation
busctl --user get-property dev.soldunov.TokenStation /dev/soldunov/TokenStation \
  dev.soldunov.TokenStation1 Snapshot
busctl --user call dev.soldunov.TokenStation /dev/soldunov/TokenStation \
  dev.soldunov.TokenStation1 Refresh
```
