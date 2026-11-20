# frob

Browser automation for AI agents: open pages, read them as an accessibility tree, and act on elements from the shell.

<p>
  <a href="https://vercel.com/labs#labs-products"><img alt="Vercel Labs Product" src="https://img.shields.io/badge/LABS-PRODUCT-0a0a0a.svg?style=for-the-badge&amp;logo=Vercel&amp;labelColor=000000" height="28"></a>
  <a href="https://www.npmjs.com/package/frob"><img alt="npm version: frob" src="https://img.shields.io/npm/v/frob.svg?style=for-the-badge&amp;labelColor=000000" height="28"></a>
  <a href="https://github.com/soulcloude00/frob/blob/main/LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/github/license/soulcloude00/frob.svg?style=for-the-badge&amp;labelColor=000000" height="28"></a>
  <a href="https://www.npmjs.com/package/frob"><img alt="npm downloads per month: frob" src="https://img.shields.io/npm/dm/frob.svg?style=for-the-badge&amp;labelColor=000000&amp;label=npm%20downloads" height="28"></a>
  <a href="https://skills.sh/soulcloude00/frob"><img alt="skills.sh" src="https://skills.sh/b/soulcloude00/frob?style=for-the-badge" height="28"></a>
</p>

frob is a native Rust CLI that gives an AI agent a browser it can drive one command at a time. `snapshot` returns the page as compact text with a ref such as `@e1` for each element. The agent acts on a ref, reads the page again to check the result, and repeats. The browser stays open between commands, so each step is one short shell call.

## Install

```bash
npm install -g frob
frob install           # download Chrome for Testing (first time only)
```

- **Homebrew:** `brew install frob`, then `frob install`.
- **Linux:** `frob install --with-deps` also installs the browser's system libraries.
- **Smaller footprint:** `frob install --headless-shell` installs the headless-only Chrome build, which is about half the size and uses less memory.
- **No install:** `npx frob open example.com`.

Project installs, building from source, `upgrade`, and `doctor` are covered in [Installation](https://github.com/soulcloude00/frob/installation).

## Quick start

```bash
frob open example.com
frob snapshot -i       # interactive elements, each with a ref
frob click @e1         # act on a ref from that snapshot
frob get url           # check the result
frob close
```

The snapshot for `example.com`:

```
- link "Learn more" [ref=e1]
```

- Refs describe the page at the time of the snapshot. After navigation or a DOM change, take a new snapshot before using refs again.
- CSS selectors, text, XPath, and semantic locators also work, for example `frob find role button click --name "Submit"`. See [Selectors](https://github.com/soulcloude00/frob/selectors).
- A click fails, instead of reporting success, when the target is hidden, disabled, outside the viewport, or covered by another element such as a consent banner. Dismiss the covering element, take a new snapshot, and retry.

The full walkthrough is in the [quick start](https://github.com/soulcloude00/frob/quick-start).

## How it works

The CLI sends each command to a background daemon that starts on the first command and controls the browser over the Chrome DevTools Protocol. Each `--session` gets its own daemon and browser. By default, the daemon closes a headless browser and exits after one hour without commands, saving state first when [session persistence](https://github.com/soulcloude00/frob/sessions) is configured.

Chrome for Testing is the default browser. You can also attach to a running Chrome with `--cdp` or `--auto-connect` ([CDP mode](https://github.com/soulcloude00/frob/cdp-mode)), select the [Lightpanda](https://github.com/soulcloude00/frob/engines/lightpanda) or [Obscura](https://github.com/soulcloude00/frob/engines/obscura) engine with `--engine`, drive Mobile Safari in the [iOS Simulator](https://github.com/soulcloude00/frob/ios), or use a cloud browser provider such as [Browserbase](https://github.com/soulcloude00/frob/providers/browserbase), [Browserless](https://github.com/soulcloude00/frob/providers/browserless), [Browser Use](https://github.com/soulcloude00/frob/providers/browser-use), [Kernel](https://github.com/soulcloude00/frob/providers/kernel), or [AgentCore](https://github.com/soulcloude00/frob/providers/agentcore).

## For AI agents

The CLI ships skills that match its version:

```bash
frob skills get core                 # the core loop, refs, waiting, and common workflows
frob skills get core --ref commands  # one reference, such as the full command list
frob skills get core --full          # adds the full command reference and templates
frob skills list                     # specialized skills: electron, slack, dogfood, and more
```

Install the discovery skill into an agent harness from [skills.sh](https://skills.sh/soulcloude00/frob):

```bash
npx skills add soulcloude00/frob
```

The installed skill is a thin stub that tells the agent to load `frob skills get core`, so its instructions always match the installed CLI. Or add this to your `AGENTS.md`:

```markdown
Use `frob` for browser automation. Before the first use, run
`frob skills get core` and follow it.
```

For MCP clients, `frob mcp` starts an MCP server over stdio. See [MCP server](https://github.com/soulcloude00/frob/commands#mcp-server).

## Commands

Run `frob --help` for a short primer, `frob --help --all` for the full list, or `frob <command> --help` for one command.

| Group | Commands | Docs |
|---|---|---|
| Navigate and read | `open`, `back`, `forward`, `reload`, `read`, `snapshot`, `get`, `is` | [Core](https://github.com/soulcloude00/frob/commands#core), [Snapshots](https://github.com/soulcloude00/frob/snapshots) |
| Interact | `click`, `dblclick`, `fill`, `type`, `press`, `keyboard`, `hover`, `focus`, `select`, `check`, `uncheck`, `scroll`, `drag`, `upload`, `mouse`, `find` | [Core](https://github.com/soulcloude00/frob/commands#core), [Find elements](https://github.com/soulcloude00/frob/commands#find-elements) |
| Wait | `wait` | [Wait](https://github.com/soulcloude00/frob/commands#wait) |
| Capture | `screenshot`, `pdf`, `record`, `download`, `diff` | [Recording](https://github.com/soulcloude00/frob/recording), [Diffing](https://github.com/soulcloude00/frob/diffing), [Files](https://github.com/soulcloude00/frob/files) |
| Page and network state | `cookies`, `storage`, `network`, `set`, `clipboard`, `eval`, `pushstate` | [Network](https://github.com/soulcloude00/frob/network), [Settings](https://github.com/soulcloude00/frob/commands#settings) |
| Tabs, frames, and dialogs | `tab`, `frame`, `dialog` | [Tabs and frames](https://github.com/soulcloude00/frob/commands#tabs-frames) |
| Debug and performance | `console`, `errors`, `highlight`, `inspect`, `trace`, `profiler`, `vitals`, `react`, `a11y` | [Debugging](https://github.com/soulcloude00/frob/debugging), [Profiler](https://github.com/soulcloude00/frob/profiler), [React](https://github.com/soulcloude00/frob/react) |
| Sessions and auth | `session`, `state`, `auth`, `profiles`, `confirm`, `deny` | [Sessions](https://github.com/soulcloude00/frob/sessions), [Security](https://github.com/soulcloude00/frob/security) |
| Agents and integrations | `skills`, `mcp`, `webmcp`, `flow`, `chat`, `batch`, `plugin` | [Skills](https://github.com/soulcloude00/frob/skills), [WebMCP](https://github.com/soulcloude00/frob/webmcp), [Plugins](https://github.com/soulcloude00/frob/plugins) |
| Observe | `stream`, `dashboard` | [Streaming](https://github.com/soulcloude00/frob/streaming), [Dashboard](https://github.com/soulcloude00/frob/dashboard) |
| Setup | `install`, `upgrade`, `doctor`, `connect`, `close` | [Installation](https://github.com/soulcloude00/frob/installation) |

Flags, environment variables, and `frob.json` are documented in [Configuration](https://github.com/soulcloude00/frob/configuration).

## Platforms

Native binaries ship for macOS (ARM64, x64), Linux (ARM64, x64, glibc and musl), and Windows (x64). The npm package picks the right binary for the host.

## Development

Building from source requires Node.js 24 or later, pnpm 11 or later, and Rust:

```bash
pnpm install
pnpm build:source               # builds the dashboard and the native CLI
pnpm link --global
frob install
```

Run the Rust checks from `cli/`:

```bash
cargo fmt -- --check
cargo clippy
cargo test
cargo test e2e -- --ignored --test-threads=1   # launches real headless Chrome
```

[AGENTS.md](AGENTS.md) lists the files to update with each change, the docs checks, and the release process.

## Documentation

- [github.com/soulcloude00/frob](https://github.com/soulcloude00/frob): introduction, guides, and reference
- [Commands](https://github.com/soulcloude00/frob/commands) and [Configuration](https://github.com/soulcloude00/frob/configuration)
- [Security](https://github.com/soulcloude00/frob/security): the auth vault, domain allowlists, action policies, and content boundaries
- [Next.js and Vercel](https://github.com/soulcloude00/frob/next) and [eve](https://github.com/soulcloude00/frob/eve): running frob in Vercel Sandbox and eve agents
- [Changelog](CHANGELOG.md)

## License

Apache-2.0
