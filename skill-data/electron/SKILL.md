---
name: electron
description: Automate Electron desktop apps (VS Code, Slack, Discord, Figma, Notion, Spotify, etc.) using frob via Chrome DevTools Protocol. Use when the user needs to interact with an Electron app, automate a desktop app, connect to a running app, control a native app, or test an Electron application. Triggers include "automate Slack app", "control VS Code", "interact with Discord app", "test this Electron app", "connect to desktop app", or any task requiring automation of a native Electron application.
allowed-tools: Bash(frob:*), Bash(npx frob:*)
---

# Electron App Automation

Automate any Electron desktop app using frob. Electron apps are built on Chromium and expose a Chrome DevTools Protocol (CDP) port that frob can connect to, enabling the same snapshot-interact workflow used for web pages.

## Core Workflow

1. **Launch** the Electron app with remote debugging enabled
2. **Connect** frob to the CDP port
3. **Snapshot** to discover interactive elements
4. **Interact** using element refs
5. **Re-snapshot** after navigation or state changes

```bash
# Launch an Electron app with remote debugging
open -a "Slack" --args --remote-debugging-port=9222

# Connect frob to the app
frob connect 9222

# Standard workflow from here
frob snapshot -i
frob click @e5
frob screenshot slack-desktop.png
```

## Launching Electron Apps with CDP

Every Electron app supports the `--remote-debugging-port` flag since it's built into Chromium.

### macOS

```bash
# Slack
open -a "Slack" --args --remote-debugging-port=9222

# VS Code
open -a "Visual Studio Code" --args --remote-debugging-port=9223

# Discord
open -a "Discord" --args --remote-debugging-port=9224

# Figma
open -a "Figma" --args --remote-debugging-port=9225

# Notion
open -a "Notion" --args --remote-debugging-port=9226

# Spotify
open -a "Spotify" --args --remote-debugging-port=9227
```

### Linux

```bash
slack --remote-debugging-port=9222
code --remote-debugging-port=9223
discord --remote-debugging-port=9224
```

### Windows

```bash
"C:\Users\%USERNAME%\AppData\Local\slack\slack.exe" --remote-debugging-port=9222
"C:\Users\%USERNAME%\AppData\Local\Programs\Microsoft VS Code\Code.exe" --remote-debugging-port=9223
```

**Important:** If the app is already running, quit it first, then relaunch with the flag. The `--remote-debugging-port` flag must be present at launch time.

## Connecting

```bash
# Connect to a specific port
frob connect 9222

# Or use --cdp on each command
frob --cdp 9222 snapshot -i

# Auto-discover a running Chromium-based app
frob --auto-connect snapshot -i
```

After `connect`, all subsequent commands target the connected app without needing `--cdp`.

## Tab Management

Electron apps often have multiple windows or webviews. Use tab commands to list and switch between them:

```bash
# List all available targets (windows, webviews, etc.)
frob tab

# Switch to a specific tab by index
frob tab 2

# Switch by URL pattern
frob tab --url "*settings*"
```

## Webview Support

Electron `<webview>` elements are automatically discovered and can be controlled like regular pages. Webviews appear as separate targets in the tab list with `type: "webview"`:

```bash
# Connect to running Electron app
frob connect 9222

# List targets -- webviews appear alongside pages
frob tab
# Example output:
#   0: [page]    Slack - Main Window     https://app.slack.com/
#   1: [webview] Embedded Content        https://example.com/widget

# Switch to a webview
frob tab 1

# Interact with the webview normally
frob snapshot -i
frob click @e3
frob screenshot webview.png
```

**Note:** Webview support works via raw CDP connection.

## Common Patterns

### Inspect and Navigate an App

```bash
open -a "Slack" --args --remote-debugging-port=9222
sleep 3  # Wait for app to start
frob connect 9222
frob snapshot -i
# Read the snapshot output to identify UI elements
frob click @e10  # Navigate to a section
frob snapshot -i  # Re-snapshot after navigation
```

### Take Screenshots of Desktop Apps

```bash
frob connect 9222
frob screenshot app-state.png
frob screenshot --full full-app.png
frob screenshot --annotate annotated-app.png
```

### Extract Data from a Desktop App

```bash
frob connect 9222
frob snapshot -i
frob get text @e5
frob snapshot --json > app-state.json
```

### Fill Forms in Desktop Apps

```bash
frob connect 9222
frob snapshot -i
frob fill @e3 "search query"
frob press Enter
frob wait 1000
frob snapshot -i
```

### Run Multiple Apps Simultaneously

Use named sessions to control multiple Electron apps at the same time:

```bash
# Connect to Slack
frob --session slack connect 9222

# Connect to VS Code
frob --session vscode connect 9223

# Interact with each independently
frob --session slack snapshot -i
frob --session vscode snapshot -i
```

## Color Scheme

The default color scheme when connecting via CDP may be `light`. To preserve dark mode:

```bash
frob connect 9222
frob --color-scheme dark snapshot -i
```

Or set it globally:

```bash
FROB_COLOR_SCHEME=dark frob connect 9222
```

## Troubleshooting

### "Connection refused" or "Cannot connect"

- Make sure the app was launched with `--remote-debugging-port=NNNN`
- If the app was already running, quit and relaunch with the flag
- Check that the port isn't in use by another process: `lsof -i :9222`

### App launches but connect fails

- Wait a few seconds after launch before connecting (`sleep 3`)
- Some apps take time to initialize their webview

### Elements not appearing in snapshot

- The app may use multiple webviews. Use `frob tab` to list targets and switch to the right one

### Cannot type in input fields

- Try `frob keyboard type "text"` to type at the current focus without a selector
- Some Electron apps use custom input components; use `frob keyboard inserttext "text"` to bypass key events

## Supported Apps

Any app built on Electron works, including:

- **Communication:** Slack, Discord, Microsoft Teams, Signal, Telegram Desktop
- **Development:** VS Code, GitHub Desktop, Postman, Insomnia
- **Design:** Figma, Notion, Obsidian
- **Media:** Spotify, Tidal
- **Productivity:** Todoist, Linear, 1Password

If an app is built with Electron, it supports `--remote-debugging-port` and can be automated with frob.
