# Session Management

Multiple isolated browser sessions with state persistence and concurrent browsing.

**Related**: [authentication.md](authentication.md) for login patterns, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Named Sessions](#named-sessions)
- [Session Isolation Properties](#session-isolation-properties)
- [Tab Pinning in a Shared Browser](#tab-pinning-in-a-shared-browser)
- [Changing settings on a running session](#changing-settings-on-a-running-session)
- [Session State Persistence](#session-state-persistence)
- [Common Patterns](#common-patterns)
- [Default Session](#default-session)
- [Session Cleanup](#session-cleanup)
- [Best Practices](#best-practices)

## Named Sessions

Use `--session` to isolate browser contexts. Agent skills should derive one stable id and reuse it on every command:

```bash
SESSION="$(frob session id --scope worktree --prefix my-skill)"
frob --session "$SESSION" --restore open https://app.example.com/login
```

`--scope worktree` uses the Git worktree root when available, then the Git root, then the canonical current directory. This is the recommended default for agents because worktrees are commonly used for parallel agent runs.

Session names accept Unicode letters and numbers, hyphens, and underscores only. For example, `café-東京_7` is valid, while `my session` is rejected because it contains a space.

```bash
# Session 1: Authentication flow
frob --session auth open https://app.example.com/login

# Session 2: Public browsing (separate cookies, storage)
frob --session public open https://example.com

# Commands are isolated by session
frob --session auth fill @e1 "user@example.com"
frob --session public get text body
```

## Session Isolation Properties

Each session has independent:
- Cookies
- LocalStorage / SessionStorage
- IndexedDB
- Cache
- Browsing history
- Open tabs

## Tab Pinning in a Shared Browser

Full isolation applies when each session launches its own browser. When sessions instead share one Chrome over `--cdp <port>`, cookies and storage are shared, and only the tab selection separates the sessions. Add `--pin-tab` so each session sticks to its own tab:

```bash
frob --session agent1 --cdp 9222 --pin-tab open https://site-a.com
frob --session agent2 --cdp 9222 --pin-tab open https://site-b.com
```

Every session remembers which tab it is bound to (by CDP target id, persisted in the session's state directory), so a restarted daemon reattaches to the session's own tab instead of adopting the most recently active one. `--pin-tab` (env `FROB_PIN_TAB=1`) additionally makes the binding strict:

- Attaching with no binding opens a fresh tab instead of adopting an existing one
- If the bound tab is closed, commands fail with a `tab_gone` error instead of silently acting on another tab. JSON output includes `"code": "tab_gone"`, `data.targetId`, and optional `data.lastUrl`
- Recovery commands still work in that state: run `tab new <url>` to bind a fresh tab, or `tab list` and switch to an existing one
- Tabs opened by other sessions or the user never steal the pinned session's active tab
- Another session cannot take a pinned tab with `tab <ref>` while the pinning session is running; the switch fails with `tab_locked` naming the owner

The flag is sticky per session: pass it once at session creation and later commands keep the strict semantics. Daemon restarts keep the pin; an explicit `close` releases it so other sessions can take the tab, and the next `--pin-tab` command pins again. Pass `--no-pin-tab` to explicitly turn the pin off again. Use each tab's `targetId` from `tab list --json` when one session needs to reference another session's tab; target ids stay stable across daemon restarts.

The structured `lastUrl` is limited to sanitized HTTP(S) URLs and `about:blank`. Credentials, query strings, and fragments are removed from HTTP(S) URLs. Opaque URLs such as `data:` are omitted. In batch JSON, the recovery object appears under `result` instead of `data`.

When re-running a shared-tab script such as the repro from #1530, add `--pin-tab` to the first command for every session. Without it, `open` intentionally preserves the legacy behavior and navigates the shared active tab, so the original script still collides. The same rule applies when sessions attach with `--auto-connect` instead of `--cdp`.

## Changing settings on a running session

Each command carries the settings you set for it (flag, environment variable, or config). The daemon compares them with the running session. A setting you omit keeps its running value, and a setting equal to the running one does nothing, so repeating `--profile ./work` on every command never relaunches the browser. Timeouts, `--action-policy`, `--confirm-actions`, `--no-auto-dialog`, and restore and pin settings apply in place. Launch settings (`--profile`, `--args`, `--headed`, `--proxy`, `--state`, `--allowed-domains`, and similar) relaunch the browser without restarting the daemon. Each change that takes effect prints one line on stderr, for example `⚠ profile: ./a → ./b (--profile), relaunched browser`. Every change appears in `lifecycle.settings.changes` with `--json`, as `applied: true` or `applied: false` (a launch setting changed while no browser is running waits for the next launch). Commands that never launch (`get cdp-url`, `session info`) apply live settings only, and `get cdp-url` sends no launch settings. Settings that only describe a browser frob launches itself (`--profile`, `--args`, `--executable-path`, `--extension`, `--proxy`, `--user-agent`, `--state`) are ignored for `--cdp`, `--auto-connect` and provider sessions and show as `effect: "ignored"`. Inherited `--state`, `--profile`, `--extension`, `--proxy` (and credentials), `--executable-path` and `--args` values are dropped when the session switches to an external browser, an empty `--provider` resets a provider session to a local launch, and `--no-auto-connect` resets an `--auto-connect` session. Secrets are redacted: passwords and CDP headers are hidden, and CDP URLs lose their credentials and query string. A launch setting is kept only after the launch succeeds, except `--allowed-domains`, which stays installed. A command denied by policy or waiting for confirmation changes no launch setting. Setting `--state` replaces an inherited `--profile` or `--extension`, and the reverse. The first command of a session still seeds from the environment the daemon was spawned with, except the connection target, which is kept only after an attach succeeds. `--state <path>` loads the file at launch only; pass `--relaunch` to relaunch the browser and load it again.

Security settings (`--allowed-domains`, `--action-policy`, `--confirm-actions`) are stored in an owner-only `<session>.settings` file in the socket directory before they take effect, so they survive a daemon restart (idle expiry, crash, kill, upgrade). The file wins over the environment when the daemon starts, a new value replaces it, `--allowed-domains ""` clears the filter, and a successful `close` (or `close --all`, which also closes sessions whose daemon is gone, under their stored policy) removes the file. A relative `--action-policy` path is stored absolute. A denied or confirmation-pending `close`, and CLI-triggered daemon restarts, keep the file. A file that cannot be read or parsed, or is readable by other users on Unix, stops the daemon from starting and names the file; `frob --session <name> close` clears it.

## Session State Persistence

### Automatic Restore

```bash
# Bare --restore uses the current --session as the persistence key
SESSION="$(frob session id --scope worktree --prefix next-dev-loop)"
frob --session "$SESSION" --restore open https://app.example.com/dashboard
```

When `--restore` or another restore key is configured, state is loaded before navigation and saved on close, daemon shutdown, idle timeout, and compatible relaunch. It is also saved periodically while the browser is open (after commands settle, at most once per `FROB_AUTOSAVE_INTERVAL_MS`, default 30000; set to `0` to save only on close), so a browser window the user closes by hand still leaves a recent save behind. A session ID by itself only isolates the daemon and does not enable persistence; without a restore key, shutdown discards transient browser state and open tabs. Idle sessions with configured persistence keep saving on the same interval, capturing changes the page makes on its own such as token refreshes. The daemon exits after one hour without commands or dashboard input by default; `--idle-timeout <time>` or `FROB_IDLE_TIMEOUT_MS` tunes this, and `0` disables it. Values too large to convert to milliseconds produce a warning and are ignored. Headed, Safari/iOS WebDriver, and user-attached browsers are exempt from the default timeout; provider-owned cloud browsers are not. The default save policy is `--restore-save auto`, which skips auto-save if restore failed or validation failed; `never` disables periodic autosave too.

```bash
frob --session "$SESSION" --restore --restore-check-url "**/dashboard" open https://app.example.com/dashboard
frob --session "$SESSION" --restore --restore-check-text Dashboard open https://app.example.com/dashboard
frob --session "$SESSION" --restore --restore-check-fn "!!localStorage.getItem('session')" open https://app.example.com/dashboard
```

Use `frob session info --json` for diagnostics:

```bash
frob --session "$SESSION" session info --json
```

### Manual State Files

Use `state save`, `state load`, and `--state <path>` when you need an explicit portable JSON file. Do not make agents construct paths under `~/.frob/sessions/`; prefer `--restore` for reusable agent sessions.

## Common Patterns

### Authenticated Session Reuse

```bash
#!/bin/bash
SESSION="$(frob session id --scope worktree --prefix app)"
frob --session "$SESSION" --restore open https://app.example.com/dashboard
```

### Concurrent Scraping

```bash
#!/bin/bash
# Scrape multiple sites concurrently

# Start all sessions
frob --session site1 open https://site1.com &
frob --session site2 open https://site2.com &
frob --session site3 open https://site3.com &
wait

# Extract from each
frob --session site1 get text body > site1.txt
frob --session site2 get text body > site2.txt
frob --session site3 get text body > site3.txt

# Cleanup
frob --session site1 close
frob --session site2 close
frob --session site3 close
```

### A/B Testing Sessions

```bash
# Test different user experiences
frob --session variant-a open "https://app.com?variant=a"
frob --session variant-b open "https://app.com?variant=b"

# Compare
frob --session variant-a screenshot /tmp/variant-a.png
frob --session variant-b screenshot /tmp/variant-b.png
```

## Default Session

When `--session` is omitted, commands use the default session:

```bash
# These use the same default session
frob open https://example.com
frob snapshot -i
frob close  # Closes default session
```

## Session Cleanup

```bash
# Close specific session
frob --session auth close

# List active sessions
frob session list
```

## Best Practices

### 1. Name Sessions Semantically

```bash
# GOOD: Clear purpose
frob --session github-auth open https://github.com
frob --session docs-scrape open https://docs.example.com

# AVOID: Generic names
frob --session s1 open https://github.com
```

### 2. Always Clean Up

```bash
# Close sessions when done
frob --session auth close
frob --session scrape close
```

### 3. Handle State Files Securely

```bash
# Don't commit state files (contain auth tokens!)
echo "*.auth-state.json" >> .gitignore

# Delete after use
rm /tmp/auth-state.json
```

### 4. Timeout Long Sessions

```bash
# Set timeout for automated scripts
timeout 60 frob --session long-task get text body
```
