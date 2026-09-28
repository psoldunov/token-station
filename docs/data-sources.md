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

This is the call behind Claude Code's `/usage`. The response carries a display-ready
`limits[]` array (`kind` = `session` | `weekly_all` | `weekly_scoped`, `percent`,
`resets_at`, `scope.model.display_name`), legacy keyed windows (`five_hour`,
`seven_day`, … with `utilization` 0–100), `spend`/`extra_usage` (paid overage) and
`seven_day_breakdown` (share per surface). The parser prefers `limits[]` and falls back
to keyed windows, so new codenamed keys are tolerated.

- Credentials: `$CLAUDE_CONFIG_DIR/.credentials.json` or `~/.claude/.credentials.json`,
  read on every poll, never written, never refreshed, never logged. Tokens need the
  `user:profile` scope (a 403 disables the endpoint for the session).
- Expired access token: the provider runs `claude auth status --json` at most every
  10 min (the CLI refreshes its own token), re-reads the file, and otherwise shows stale
  data with "Open Claude Code to refresh it".
- Polling: default every 300 s, never faster than 180 s (forced refreshes: 30 s floor);
  429 honours `Retry-After`, else exponential backoff to 1 h.
- **Why the User-Agent:** without the CLI's User-Agent the endpoint rate-limits
  aggressively (widely reported by other monitors). It is configurable
  (`claude.user_agent`). This endpoint is not a documented API and may change.

### Plan limits: statusline (official)

Claude Code pipes JSON to the `statusLine` command; Pro/Max sessions include
`rate_limits.five_hour` / `seven_day` (`used_percentage`, `resets_at` epoch seconds)
after the first response. `token-station statusline [--wrap CMD]` forwards it to the
daemon (`IngestClaudeStatusline`) and passes stdin to the wrapped command so an
existing statusline keeps rendering. The newer of statusline and endpoint observation
wins per window.

### Tokens

`~/.claude/projects/**/*.jsonl` (and `~/.config/claude/projects`): assistant entries
with `message.usage`; de-duplicated on `message.id:requestId`; incremental scan of the
last 8 days.

## Codex

### Plan limits and account tokens: `codex app-server`

Newline-delimited JSON-RPC over stdio (no `"jsonrpc"` field). Handshake `initialize`
(`clientInfo`) + `initialized`, then:

- `account/read` → account type (`chatgpt` | `apiKey` | `amazonBedrock`) and `planType`
- `account/rateLimits/read` (`excludeResetCreditDetails: true`) → `rateLimitsByLimitId`
  snapshots with `primary`/`secondary` `{usedPercent, windowDurationMins, resetsAt}`,
  credits, reset-credit count. A Pro account may expose only a weekly window.
- `account/usage/read` → account-wide `dailyUsageBuckets` and lifetime tokens.

Reads do not consume quota. The first account request after spawn takes ~6 s, later
ones ~0.6 s; the child is stopped after `linger_secs` idle.

### Fallback: ChatGPT usage endpoint

When app-server is unavailable, `GET https://chatgpt.com/backend-api/wham/usage` with
the access token and account id from `$CODEX_HOME/auth.json` (read-only, never
refreshed). Undocumented; parsed leniently.

### Tokens

`$CODEX_HOME/sessions/**/rollout-*.jsonl` and `archived_sessions/`: `token_count` events
(`info.last_token_usage`), counted only when `total_token_usage` grows (Codex repeats
events); model from the latest `turn_context`. The latest embedded `rate_limits`
snapshot is a last-resort offline source for the windows.

## Pricing

API-equivalent cost uses LiteLLM's `model_prices_and_context_window.json`: a vendored
subset (`data/pricing/litellm-subset.json`) plus a daily refresh cached in
`$XDG_CACHE_HOME/token-station/pricing.json` (`pricing.auto_update`). Long-context (>200k
prompt) tiers apply per request; unknown models are counted but not priced.
