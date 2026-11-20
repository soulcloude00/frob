# Authentication Patterns

Login flows, session persistence, OAuth, 2FA, and authenticated browsing.

**Related**: [session-management.md](session-management.md) for state persistence details, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Import Auth from Your Browser](#import-auth-from-your-browser)
- [Persistent Profiles](#persistent-profiles)
- [Session Persistence](#session-persistence)
- [Basic Login Flow](#basic-login-flow)
- [Plugins](#plugins)
- [Saving Authentication State](#saving-authentication-state)
- [Restoring Authentication](#restoring-authentication)
- [OAuth / SSO Flows](#oauth--sso-flows)
- [Two-Factor Authentication](#two-factor-authentication)
- [HTTP Basic Auth](#http-basic-auth)
- [Cookie-Based Auth](#cookie-based-auth)
- [Token Refresh Handling](#token-refresh-handling)
- [Security Best Practices](#security-best-practices)

## Import Auth from Your Browser

The fastest way to authenticate is to reuse cookies from a Chrome session you are already logged into.

**Step 1: Start Chrome with remote debugging**

```bash
# macOS
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --remote-debugging-port=9222

# Linux
google-chrome --remote-debugging-port=9222

# Windows
"C:\Program Files\Google\Chrome\Application\chrome.exe" --remote-debugging-port=9222
```

Log in to your target site(s) in this Chrome window as you normally would.

> **Security note:** `--remote-debugging-port` exposes full browser control on localhost. Any local process can connect and read cookies, execute JS, etc. Only use on trusted machines and close Chrome when done.

**Step 2: Grab the auth state**

```bash
# Auto-discover the running Chrome and save its cookies + localStorage
frob --auto-connect state save ./my-auth.json
```

**Step 3: Reuse in automation**

```bash
# Load auth at launch
frob --state ./my-auth.json open https://app.example.com/dashboard

# Or load into an already-launched session
frob open about:blank
frob state load ./my-auth.json
frob open https://app.example.com/dashboard
```

This works for any site, including those with complex OAuth flows, SSO, or 2FA, as long as Chrome already has valid session cookies.

Saved localStorage and sessionStorage are written through Chrome DevTools, not page JavaScript, and only while the page is on the saved origin. If an origin redirects elsewhere or fails to load, its storage is skipped, the rest still loads, and `state load` lists it under `skippedOrigins`. For `--state` and `--restore` at launch, `session info` lists it under `restoreSkippedOrigins`.

> **Security note:** State files contain session tokens in plaintext. Add them to `.gitignore`, delete when no longer needed, and set `FROB_ENCRYPTION_KEY` for encryption at rest. See [Security Best Practices](#security-best-practices).

**Tip:** Combine with `--session <id> --restore` so the imported auth auto-persists across restarts:

```bash
SESSION="$(frob session id --scope worktree --prefix myapp)"
frob --session "$SESSION" --restore --state ./my-auth.json open https://app.example.com/dashboard
# From now on, state is auto-saved/restored for this session
```

## Persistent Profiles

Use `--profile` to point frob at a Chrome user data directory. This persists everything (cookies, IndexedDB, service workers, cache) across browser restarts without explicit save/load:

```bash
# First run: login once
frob --profile ~/.myapp-profile open https://app.example.com/login
# ... complete login flow ...

# All subsequent runs: already authenticated
frob --profile ~/.myapp-profile open https://app.example.com/dashboard
```

Use different paths for different projects or test users:

```bash
frob --profile ~/.profiles/admin open https://app.example.com
frob --profile ~/.profiles/viewer open https://app.example.com
```

Or set via environment variable:

```bash
export FROB_PROFILE=~/.myapp-profile
frob open https://app.example.com/dashboard
```

## Session Persistence

Use `--restore` with a stable `--session` to auto-save and restore cookies + localStorage without managing files:

```bash
# Auto-saves state on close, auto-restores on next launch
SESSION="$(frob session id --scope worktree --prefix twitter)"
frob --session "$SESSION" --restore open https://twitter.com
# ... login flow ...
frob --session "$SESSION" --restore close  # state saved to ~/.frob/sessions/

# Next time: state is automatically restored
frob --session "$SESSION" --restore open https://twitter.com
```

Encrypt state at rest:

```bash
export FROB_ENCRYPTION_KEY=$(openssl rand -hex 32)
frob --session secure --restore open https://app.example.com
```

## Basic Login Flow

```bash
# Navigate to login page
frob open https://app.example.com/login
frob wait --load domcontentloaded
frob wait --fn "(() => { const usable = (el) => { const style = getComputedStyle(el); return !el.disabled && !el.readOnly && style.display !== 'none' && style.visibility !== 'hidden' && el.getClientRects().length > 0; }; const username = Array.from(document.querySelectorAll('input[type=email], input[type=text], input[autocomplete=username], input[name*=email i], input[name*=user i], input[name*=login i]')).some(usable); const password = Array.from(document.querySelectorAll('input[type=password]')).some(usable); return username && password; })()"

# Get form elements
frob snapshot -i
# - textbox "Email" [ref=e1]
# - textbox "Password" [ref=e2]
# - button "Sign In" [ref=e3]

# Fill credentials
frob fill @e1 "user@example.com"
frob fill @e2 "password123"

# Submit
frob click @e3
frob wait --url "**/dashboard"

# Verify login succeeded
frob get url  # Should be dashboard, not login
```

After submitting, wait for the authenticated destination, a success message, or another app-specific condition. Do not use `networkidle` as a generic login wait because long-lived background connections can keep it from resolving.

`auth login` filters matching controls by their layout size, computed visibility and opacity, and disabled/readonly state, including custom CSS selectors. If a selected credential field is replaced or redirects focus before entry, the command fails without submitting.

For a form reached through an in-page click, challenge clearance, consent dismissal, or another stateful step, use the auth vault without discarding the prepared document:

```bash
frob open https://app.example.com/
frob click "a[href='/login']"
frob auth login my-app --no-navigate
```

`--no-navigate` suppresses only the initial navigation that `auth login` normally performs. An existing active top-level HTTP(S) page is required. Before either credential is filled, frob compares the page origin with the effective credential URL using scheme, host, and effective port. Paths, queries, and fragments may differ. Selector waits, credential filling, submit clicking, and submit-triggered navigation remain unchanged.

The command-level `--url` override takes precedence over stored profile or credential-provider URL metadata. In no-navigation mode it is an expected-origin constraint, not a navigation destination. This supports hosted identity-provider flows:

```bash
frob auth login work --no-navigate --url https://identity.example.com/login
```

## Plugins

Use credential provider plugins when credentials live in external vault software. Plugins are configured in `~/.frob/config.json` or a file loaded with `--config` and run as external executables over the `frob.plugin.v1` stdio JSON protocol. The auto-discovered `./frob.json` ignores `plugins` with a warning. An explicitly loaded file replaces default config discovery.

Add a plugin with `plugin add`, which writes `~/.frob/config.json` by default; use `--project` for `./frob.json` (only used with `--config`). A plain `name` or `@scope/name` resolves from npm; `owner/repo` resolves from GitHub:

```bash
frob plugin add frob-plugin-vault --name vault
frob plugin add @company/frob-plugin-vault --name vault
frob plugin add org/frob-plugin-cloud-browser
```

```json
{
  "plugins": [
    {
      "name": "vault",
      "command": "frob-plugin-vault",
      "capabilities": ["credential.read"]
    },
    {
      "name": "cloud-browser",
      "command": "frob-plugin-cloud-browser",
      "capabilities": ["browser.provider"]
    },
    {
      "name": "stealth",
      "command": "frob-plugin-stealth",
      "capabilities": ["launch.mutate"]
    },
    {
      "name": "captcha",
      "command": "frob-plugin-captcha",
      "capabilities": ["command.run", "captcha.solve"]
    }
  ]
}
```

Inspect configured plugins before use:

```bash
frob plugin list
frob plugin show vault
```

Resolve credentials just-in-time for one login:

```bash
frob auth login my-app --credential-provider vault --item "My App"
```

After stateful setup, combine the provider with no-navigation mode. The provider secret remains in memory and does not enter generated process arguments or normal output:

```bash
frob auth login my-app --credential-provider vault --item "My App" --no-navigate --url https://identity.example.com/login
```

Use a plugin as a browser provider or a generic domain command:

```bash
frob --provider cloud-browser open https://example.com
frob plugin run captcha captcha.solve --payload '{"siteKey":"...","url":"https://example.com"}'
```

`plugin run` is for `command.run` and custom capabilities. Core capabilities and protocol request types use their dedicated command paths.

Credential resolution, browser launch and cleanup, and launch mutation errors omit plugin response error text and unsupported protocol values from diagnostics. Manifest discovery and `plugin run` retain detailed errors. Plugins remain trusted local executables; this redaction does not sandbox them.

Use `--url`, `--username-selector`, `--password-selector`, and `--submit-selector` on `auth login` to override plugin-provided metadata for the current login only.

Gate plugin secret access separately from normal login automation:

```bash
frob --confirm-actions plugin:vault:credential.read auth login my-app --credential-provider vault --item "My App"
frob --confirm-actions plugin:cloud-browser:browser.provider --provider cloud-browser open https://example.com
frob --confirm-actions plugin:stealth:launch.mutate open https://example.com
```

Do not put vault tokens or passwords in plugin command args. Use the vault vendor's own login/session mechanism or environment outside frob config.

## Saving Authentication State

After logging in, save state for reuse:

```bash
# Login first (see above)
frob open https://app.example.com/login
frob snapshot -i
frob fill @e1 "user@example.com"
frob fill @e2 "password123"
frob click @e3
frob wait --url "**/dashboard"

# Save authenticated state
frob state save ./auth-state.json
```

## Restoring Authentication

Skip login by loading saved state:

```bash
# Load saved auth state
frob state load ./auth-state.json

# Navigate directly to protected page
frob open https://app.example.com/dashboard

# Verify authenticated
frob snapshot -i
```

## OAuth / SSO Flows

For OAuth redirects:

```bash
# Start OAuth flow
frob open https://app.example.com/auth/google

# Handle redirects automatically
frob wait --url "**/accounts.google.com**"
frob snapshot -i

# Fill Google credentials
frob fill @e1 "user@gmail.com"
frob click @e2  # Next button
frob wait 2000
frob snapshot -i
frob fill @e3 "password"
frob click @e4  # Sign in

# Wait for redirect back
frob wait --url "**/app.example.com**"
frob state save ./oauth-state.json
```

## Two-Factor Authentication

Handle 2FA with manual intervention:

```bash
# Login with credentials
frob open https://app.example.com/login --headed  # Show browser
frob snapshot -i
frob fill @e1 "user@example.com"
frob fill @e2 "password123"
frob click @e3

# Wait for user to complete 2FA manually
echo "Complete 2FA in the browser window..."
frob wait --url "**/dashboard" --timeout 120000

# Save state after 2FA
frob state save ./2fa-state.json
```

## HTTP Basic Auth

For sites using HTTP Basic Authentication:

```bash
# Set credentials before navigation
frob set credentials username password

# Navigate to protected resource
frob open https://protected.example.com/api
```

The credentials also apply to tabs opened later through `tab new` or `click --new-tab`, including their first document request.

## Cookie-Based Auth

Manually set authentication cookies:

```bash
# Set auth cookie
frob cookies set session_token "abc123xyz"

# Navigate to protected page
frob open https://app.example.com/dashboard
```

## Token Refresh Handling

For sessions with expiring tokens:

```bash
#!/bin/bash
# Wrapper that handles token refresh

STATE_FILE="./auth-state.json"

# Try loading existing state
if [[ -f "$STATE_FILE" ]]; then
    frob state load "$STATE_FILE"
    frob open https://app.example.com/dashboard

    # Check if session is still valid
    URL=$(frob get url)
    if [[ "$URL" == *"/login"* ]]; then
        echo "Session expired, re-authenticating..."
        # Perform fresh login
        frob snapshot -i
        frob fill @e1 "$USERNAME"
        frob fill @e2 "$PASSWORD"
        frob click @e3
        frob wait --url "**/dashboard"
        frob state save "$STATE_FILE"
    fi
else
    # First-time login
    frob open https://app.example.com/login
    # ... login flow ...
fi
```

## Security Best Practices

1. **Never commit state files** - They contain session tokens
   ```bash
   echo "*.auth-state.json" >> .gitignore
   ```

2. **Use environment variables for credentials**
   ```bash
   frob fill @e1 "$APP_USERNAME"
   frob fill @e2 "$APP_PASSWORD"
   ```

3. **Clean up after automation**
   ```bash
   frob cookies clear
   rm -f ./auth-state.json
   ```

4. **Use short-lived sessions for CI/CD**
   ```bash
   # Don't persist state in CI
   frob open https://app.example.com/login
   # ... login and perform actions ...
   frob close  # Session ends, nothing persisted
   ```
