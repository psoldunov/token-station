# Socket API (macOS)

macOS has no session bus, so on macOS the daemon serves the interface from
[docs/dbus-api.md](dbus-api.md) over a Unix-domain socket instead. The methods, their
arguments and their semantics are the same; only the framing differs. Linux keeps D-Bus.

The menu bar app (`frontends/macos`) and the CLI (`token-station status`, `refresh`,
`statusline`) are the clients.

## Where it listens

```
~/Library/Application Support/dev.soldunov.TokenStation/daemon.sock
```

The daemon keeps the directory at mode `0700` and the socket at `0600`, and it rejects
any connection whose peer user id differs from its own. `$TOKEN_STATION_SOCKET`
overrides the path for every component (daemon, CLI, app), which is how tests and
development builds keep out of the way of a real daemon.

Only one daemon runs per socket: it holds an exclusive `flock` on a lock file named
after the socket (`daemon.lock` by default) for its whole life. A second `token-station daemon` exits with status 75
(`EX_TEMPFAIL`), like it does on Linux when the bus name is taken. A socket file left
behind by a daemon that crashed is removed by the next one, once it holds the lock.

## Framing

The connection carries [JSON-RPC 2.0](https://www.jsonrpc.org/specification)
messages, one per line: UTF-8 JSON with no embedded newline, terminated by `\n`. A line
may carry at most 1 MiB before its newline. A longer one is answered with an
`invalid request` error and the connection is closed. Requests on one connection may be answered out of order, so
clients match responses by `id`.

```jsonc
// client → daemon
{"jsonrpc": "2.0", "id": 1, "method": "GetHistory",
 "params": {"provider": "claude", "windowId": "session", "since": 1790510400}}
// daemon → client
{"jsonrpc": "2.0", "id": 1, "result": [[1790596740, 34.0], [1790597040, 35.0]]}
```

## Methods

| Method | Params | Result |
|--------|--------|--------|
| `GetVersion` | – | `{"version": "0.1.0", "protocol": 1}` |
| `GetSnapshot` | – | `{"revision": 42, "snapshot": {…}}` |
| `Subscribe` | – | Same as `GetSnapshot`. From then on the daemon sends this connection the notifications below. |
| `Refresh` | – | `null`, once the refresh has finished. Concurrent calls share one refresh, as on D-Bus. |
| `GetHistory` | `{"provider": "claude", "windowId": "session", "since": 1790510400}` | `[[ts, percent], …]`, oldest first, at most 120 points. Unknown provider or window gives `[]`. |
| `GetSettings` | – | The config as a JSON object: snake_case keys, the structure of `config.toml`. |
| `SetSettings` | `{"settings": {…}}` | `null`. Replaces the config; missing keys take their defaults. |
| `IngestClaudeStatusline` | `{"json": "<raw statusline JSON text>"}` | `null`. At most 64 KiB of text. |

`snapshot` is the object described in [docs/dbus-api.md](dbus-api.md#snapshot-json) and
`data/snapshot.schema.json`; `revision` repeats `snapshot.revision`. `since` is Unix
seconds.

## Notifications

After `Subscribe`, the daemon pushes notifications (JSON-RPC requests without an `id`):

```jsonc
{"jsonrpc": "2.0", "method": "SnapshotChanged",
 "params": {"revision": 43, "snapshot": {…}}}

{"jsonrpc": "2.0", "method": "Alert",
 "params": {"provider": "claude", "providerName": "Claude Code",
            "windowId": "session", "windowLabel": "Session",
            "kind": "warning",            // warning | critical | reset
            "percent": 82.4, "resetsAt": 1790614800,   // resetsAt may be null
            "summary": "Claude Code: Session at 82 %", "body": "Resets in 2 h 14 min"}}
```

`SnapshotChanged` always carries the latest state. A client that reads slowly may miss
intermediate revisions, never the newest one. `Alert` is sent when a plan window crosses
the warning or critical threshold, or resets (when `notify_on_reset` is on), and only
while `alerts.notify` is on. The macOS app shows it with the system's notification
center and words it itself from the structured fields; `summary` and `body` are the
daemon's own English text for clients that do not, written when the alert was raised.
Alerts raised while nobody is subscribed are kept (up to 16) and delivered to the next
subscriber, so a countdown in a kept alert's `body` can be out of date by then;
`resetsAt` never is.

## Errors

Standard JSON-RPC error objects:

| Code | Meaning |
|------|---------|
| `-32700` | The line is not JSON. Answered with `"id": null`; the connection stays open. |
| `-32600` | Not a JSON-RPC request, or a line over 1 MiB (the connection is closed). |
| `-32601` | Unknown method. |
| `-32602` | Invalid params, including every case D-Bus reports as `InvalidArgs`: a `SetSettings` document that is out of bounds, has an unknown key, is over 256 KiB, or targets a `config.toml` that is read-only or a symlink (Nix/home-manager manages it); an `IngestClaudeStatusline` payload that is too large or not a statusline. For `SetSettings`, `error.data.problems` lists every problem. |
| `-32000` | The request was valid but the daemon could not carry it out, for example a settings write that failed on a full disk (D-Bus `Failed`). |
| `-32603` | Internal error. |

## Example

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"GetSnapshot"}' \
  | nc -U ~/Library/Application\ Support/dev.soldunov.TokenStation/daemon.sock
```
