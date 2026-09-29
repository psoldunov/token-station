# Data sources

Verified on 2026-09-28 against Claude Code 2.1.283 (Max 20x) and codex-cli 0.157.0
(ChatGPT Pro). Sanitized captures live in `crates/ts-claude/tests/fixtures` and
`crates/ts-codex/tests/fixtures`.

## Claude Code

### Plan limits: OAuth usage endpoint (undocumented)

`GET https://api.anthropic.com/api/oauth/usage` with

```
Authorization: Bearer <claudeAiOauth.accessToken from .credentials.json>
anthropic-beta: oauth-2025-04-20
User-Agent: claude-code/<installed version>
```

Claude Code's `/usage` command makes this same call. The response has a display-ready
`limits[]` array (`kind` = `session` | `weekly_all` | `weekly_scoped`, `percent`,
`resets_at`, `scope.model.display_name`), the older keyed windows (`five_hour`,
`seven_day`, … with `utilization` 0–100), `spend`/`extra_usage` (paid overage) and
`seven_day_breakdown` (share per surface). The parser prefers `limits[]` and falls back
to the keyed windows, so new codenamed keys don't break it.

- Credentials: `$CLAUDE_CONFIG_DIR/.credentials.json` or `~/.claude/.credentials.json`.
  The file is read on every poll and is never written, refreshed or logged. The token
  needs the `user:profile` scope; a 403 turns the endpoint off for the rest of the
  session.
- Expired access token: at most once every 10 min, the provider runs
  `claude auth status --json`, which makes the CLI refresh its own token, and then
  re-reads the file. If that doesn't help, it shows the stale data with
  "Open Claude Code to refresh it".
- Polling: every 300 s by default. `claude.min_endpoint_interval_secs` sets the shortest
  gap between two endpoint calls (default 180 s, minimum 120 s). Forced refreshes use a
  30 s minimum instead. On a 429 the provider honours `Retry-After`, or backs off
  exponentially up to 1 h if there isn't one.
- User-Agent: without the CLI's User-Agent the endpoint rate-limits hard, and other
  usage monitors that call it have hit the same problem. You can change the value with
  `claude.user_agent`. Keep in mind this endpoint is not a documented API and may change.

### Plan limits: statusline (official)

Claude Code pipes JSON to the `statusLine` command. In Pro/Max sessions that JSON
includes `rate_limits.five_hour` / `seven_day` (`used_percentage`, `resets_at` epoch
seconds) once the first response has come back. `token-station statusline [--wrap CMD]`
forwards it to the daemon (`IngestClaudeStatusline`) and passes stdin on to the wrapped
command, so an existing statusline keeps rendering. For each window, whichever
observation is newer wins, statusline or endpoint.

### Tokens

`~/.claude/projects/**/*.jsonl` (and `~/.config/claude/projects`): assistant entries
with `message.usage`, de-duplicated on `message.id:requestId`. The scan is incremental
and covers the last 8 days.

## Codex

### Plan limits and account tokens: `codex app-server`

Newline-delimited JSON-RPC over stdio (no `"jsonrpc"` field). The handshake is
`initialize` (`clientInfo`) followed by `initialized`, then:

- `account/read` → account type (`chatgpt` | `apiKey` | `amazonBedrock`) and `planType`
- `account/rateLimits/read` (`excludeResetCreditDetails: true`) → `rateLimitsByLimitId`
  snapshots with `primary`/`secondary` `{usedPercent, windowDurationMins, resetsAt}`,
  credits, reset-credit count. A Pro account may expose only a weekly window.
- `account/usage/read` → account-wide `dailyUsageBuckets` and lifetime tokens.

Reads do not use up quota. The first account request after spawn takes about 6 s and
later ones about 0.6 s. The daemon stops the child after `linger_secs` of idleness.

### Fallback: ChatGPT usage endpoint

When app-server isn't available, the provider calls
`GET https://chatgpt.com/backend-api/wham/usage` with the access token and account id
from `$CODEX_HOME/auth.json`. It only reads that file and never refreshes the token.
This endpoint is undocumented too, so the parser is lenient.

### Tokens

`$CODEX_HOME/sessions/**/rollout-*.jsonl` and `archived_sessions/`: `token_count` events
(`info.last_token_usage`). Codex repeats these events, so one only counts when
`total_token_usage` has grown. The model comes from the latest `turn_context`. If
nothing else is available offline, the latest `rate_limits` snapshot embedded in the
rollout fills in the windows.

## Pricing

API-equivalent cost uses LiteLLM's `model_prices_and_context_window.json`: a vendored
subset (`data/pricing/litellm-subset.json`) plus a daily refresh cached in
`$XDG_CACHE_HOME/token-station/pricing.json` (`pricing.auto_update`). Long-context tiers
(over 200k prompt tokens) apply per request. Unknown models are counted but not priced.
