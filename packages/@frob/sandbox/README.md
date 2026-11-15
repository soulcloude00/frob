# @frob/sandbox

Helpers for installing and running `frob` inside sandbox runtimes.

This package does not define model tools. Use it from framework-specific tools, agents, route handlers, or jobs that already decide what the browser should do.

## eve

For eve agents, use [`@frob/eve`](../eve/). It mounts the full browser tool set and exposes the sandbox bootstrap helpers from `@frob/eve/sandbox`, with this package as an internal dependency.

## Vercel Sandbox

Install `@vercel/sandbox` in the consuming app:

```bash
pnpm add @vercel/sandbox
```

Then use the Vercel provider entry:

```ts
import { runFrobCommand, withFrobSandbox } from "@frob/sandbox/vercel";

const snapshot = await withFrobSandbox(async (sandbox) => {
  await runFrobCommand(sandbox, ["open", "https://example.com"]);
  const result = await runFrobCommand(sandbox, ["snapshot", "-i", "-c"], {
    json: false,
  });
  return result.stdout;
});
```

The Vercel helpers install browser system dependencies by default. Pass `installSystemDependencies: false` only when the sandbox image already provides Chromium's required libraries.

Set `FROB_SNAPSHOT_ID` to boot from a prebuilt Vercel Sandbox snapshot. Without a snapshot, the helper installs system dependencies, `frob`, and Chrome on first boot.

### One MCP server per sandbox

`startFrobMcp` keeps one `frob mcp` running over stdin, so tool calls reuse the same process instead of starting a command each time. It needs `@vercel/sandbox` 3.7 or later.

```ts
import { createFrobSandbox, startFrobMcp } from "@frob/sandbox/vercel";

const sandbox = await createFrobSandbox();
const mcp = await startFrobMcp(sandbox, { timeout: 60_000 });
await mcp.callTool("frob_open", { url: "https://example.com" });
const snapshot = await mcp.callTool("frob_snapshot", { interactive: true });
await mcp.close();
await sandbox.stop();
```

`timeout` applies to each request and has no default. For another runtime, pass a process with `stdin` and `stdout` streams to `connectFrobMcp` from `@frob/sandbox`.

## MCP tool catalog

`@frob/sandbox/mcp-tools.json` lists every `frob mcp` tool with its title, description, input schema, annotations, and startup profiles, generated from the CLI of the same version. Use it to build tools for runtimes that cannot run `tools/list`, and call them with `frob mcp call <tool> '<json>'`. Run `pnpm mcp-tools:sync` after changing MCP tools; a Rust test fails while the file is out of date.

## Version pinning

By default, this package installs the `frob` version that matches its own, such as `frob@0.41.0` for `@frob/sandbox@0.41.0`.

Pass `installSpec: "latest"` or another npm spec to override that default.
