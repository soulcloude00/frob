import type { EvalCase } from "../lib/types.ts";

const RUBRIC = `
1 - Agent does not produce valid frob commands
2 - Agent uses frob but with wrong commands or missing steps
3 - Agent uses correct commands but skips the snapshot-interact workflow
4 - Agent follows the correct workflow with appropriate commands
5 - Agent follows the optimal workflow: navigate, snapshot, interact with refs, re-snapshot as needed
`.trim();

const COMMAND_CONTEXT = `You already ran \`frob skills get core\` and loaded these commands:
- frob open <url> (navigate to a page)
- frob snapshot -i (get interactive elements with refs like @e1, @e2)
- frob click @ref (click element)
- frob fill @ref "text" (clear and type)
- frob type @ref "text" (type without clearing)
- frob select @ref "option" (select dropdown)
- frob screenshot (screenshot to temp dir)
- frob screenshot --full (full page screenshot)
- frob diff url <url1> <url2> (compare two pages)
- frob diff snapshot (compare current vs last snapshot)
- frob state save ./file.json (save auth state)
- frob state load ./file.json (restore auth state)
- frob get text @ref (get element text)
- frob wait <selector|ms> (wait for element or time)
- frob --session-name <name> open <url> (named session with auto-save)

Workflow: open -> snapshot -i -> interact with refs -> re-snapshot after changes.`;

export const cases: EvalCase[] = [
  {
    id: "cu-01",
    name: "Navigate and screenshot workflow",
    category: "command-usage",
    prompt: "Open example.com and take a screenshot",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate)",
      "frob\\s+screenshot",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-02",
    name: "Form filling workflow",
    category: "command-usage",
    prompt:
      "Go to example.com/signup, fill in name as 'Jane Doe' and email as 'jane@test.com', then submit",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate)",
      "frob\\s+snapshot",
      "frob\\s+(fill|type)",
      "frob\\s+(click|press|key)",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-03",
    name: "Snapshot with element refs",
    category: "command-usage",
    prompt: "Get all interactive elements on example.com",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate)",
      "frob\\s+snapshot",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-04",
    name: "Diff comparison workflow",
    category: "command-usage",
    prompt:
      "Compare the homepage of staging.example.com and prod.example.com",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+diff|staging\\.example\\.com.*prod\\.example\\.com",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-05",
    name: "Authentication with state persistence",
    category: "command-usage",
    prompt:
      "Log into app.example.com, then save the auth state for future sessions",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate)",
      "state\\s+save|--session-name|auth\\s+save",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-06",
    name: "Data extraction workflow",
    category: "command-usage",
    prompt:
      "Extract the text content of the main heading on example.com",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate)",
      "snapshot|get\\s+text",
    ],
    rubric: RUBRIC,
  },
  {
    id: "cu-07",
    name: "Full-page screenshot",
    category: "command-usage",
    prompt: "Take a full-page screenshot of example.com",
    context: COMMAND_CONTEXT,
    expectedPatterns: [
      "frob\\s+(open|goto|navigate|screenshot)",
      "screenshot.*--full",
    ],
    rubric: RUBRIC,
  },
];
