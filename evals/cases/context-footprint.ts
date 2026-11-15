import type { EvalCase } from "../lib/types.ts";

const RUBRIC = `
1 - Agent ignores the distinction between CLI skill context and MCP tool discovery
2 - Agent mentions both surfaces but misses the required CLI skills workflow
3 - Agent identifies the CLI skills workflow and MCP tools/list, but misses pagination
4 - Agent correctly gathers CLI skill context and MCP discovery context
5 - Agent gathers both surfaces, explains that MCP discovery is paginated, and avoids collapsing MCP to a generic argv tool
`.trim();

const CONTEXT = `
Facts for this eval:
- CLI agents start from the thin installed skill at skills/frob/SKILL.md.
- That thin skill is only a discovery stub. The agent must run frob skills list and frob skills get core or frob skills get core --full to load current workflow and command content.
- frob skills get core --full includes the command reference.
- MCP clients learn the surface through initialize and tools/list.
- frob mcp defaults to the core startup tools profile to reduce context.
- Use frob mcp --tools all to expose the full typed CLI parity surface.
- Use frob mcp --tools core,network,react to compose startup profiles.
- tools/list is paginated with nextCursor, so modern clients can load large typed surfaces incrementally.
- MCP tool calls return structuredContent, and the server exposes typed tools rather than one generic argv tool.
- From a shell, MCP discovery can be probed by piping JSON-RPC lines to frob mcp.
- Example MCP probes:
  printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n' | frob mcp
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n' | frob mcp
  printf '{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}\n' | frob mcp --tools all
  printf '{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"cursor":"64"}}\n' | frob mcp --tools all
`.trim();

export const cases: EvalCase[] = [
  {
    id: "cf-01",
    name: "Compares CLI skill context with MCP discovery context",
    category: "context-footprint",
    prompt:
      "Measure how much context an agent needs to learn frob through the CLI skill workflow versus MCP discovery. Show the shell commands or JSON-RPC probes you would run.",
    context: CONTEXT,
    expectedPatterns: [
      "frob\\s+skills\\s+list",
      "frob\\s+skills\\s+get\\s+core",
      "frob\\s+skills\\s+get\\s+core\\s+--full",
      "--tools\\s+all",
      "tools/list",
      "initialize",
      "nextCursor|cursor|paginated|pagination",
    ],
    forbiddenPatterns: [
      "frob\\s+tools\\s+list",
      "single\\s+generic\\s+(argv|args)\\s+tool|one\\s+generic\\s+(argv|args)\\s+tool",
    ],
    rubric: RUBRIC,
  },
];
