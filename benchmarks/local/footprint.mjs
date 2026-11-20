// Context footprint: tokens an agent reads to see one page, for frob, agent-browser, and
// Playwright MCP, on the same pages and the same Chrome binary.
//
//   node footprint.mjs [--frob <path>] [--out <dir>]
//
// Writes each tool's raw output to <out>/<tool>/<page>.<mode>.txt and a summary to
// <out>/footprint.json. Tokens are counted with the o200k tokenizer; Claude's tokenizer gives
// different absolute numbers but the same ratios to within a few percent.

import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';
import { countTokens } from 'gpt-tokenizer/encoding/o200k_base';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { PAGES, chromePath } from './pages.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const FROB = resolve(arg('--frob', join(here, '../../cli/target/release/frob')));
const AGENT_BROWSER = join(here, 'node_modules/.bin/agent-browser');
const OUT = resolve(arg('--out', join(here, 'results/footprint')));
const CHROME = chromePath();

function cli(bin, args, extra = []) {
  try {
    return execFileSync(bin, [...extra, ...args], { encoding: 'utf8', timeout: 90_000, stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (e) {
    return (e.stdout || '') + (e.stderr || '');
  }
}

async function cliTool(name, bin, extra) {
  const results = {};
  for (const page of PAGES) {
    cli(bin, ['open', page.url], extra);
    cli(bin, ['wait', '--load', 'networkidle'], extra);
    results[page.name] = {
      full: cli(bin, ['snapshot'], extra),
      interactive: cli(bin, ['snapshot', '-i'], extra),
    };
  }
  cli(bin, ['close'], extra);
  return results;
}

async function playwrightMcp() {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [join(here, 'node_modules/@playwright/mcp/cli.js'), '--headless', '--isolated', '--executable-path', CHROME],
    stderr: 'ignore',
  });
  const client = new Client({ name: 'footprint', version: '1.0.0' });
  await client.connect(transport);
  const text = (r) => r.content.filter((c) => c.type === 'text').map((c) => c.text).join('\n');
  const results = {};
  for (const page of PAGES) {
    await client.callTool({ name: 'browser_navigate', arguments: { url: page.url } });
    await client.callTool({ name: 'browser_wait_for', arguments: { time: 2 } });
    // Playwright MCP has one snapshot mode: the aria snapshot with refs and text.
    const snap = text(await client.callTool({ name: 'browser_snapshot', arguments: {} }));
    results[page.name] = { full: snap, interactive: snap };
  }
  await client.close();
  return results;
}

const tools = {
  frob: () => cliTool('frob', FROB, ['--session', 'footprint', '--executable-path', CHROME]),
  'agent-browser': () => cliTool('agent-browser', AGENT_BROWSER, ['--session', 'footprint', '--executable-path', CHROME]),
  'playwright-mcp': playwrightMcp,
};

const only = arg('--tools', Object.keys(tools).join(',')).split(',');
const summary = {};
for (const name of only) {
  process.stderr.write(`${name}...\n`);
  const results = await tools[name]();
  mkdirSync(join(OUT, name), { recursive: true });
  summary[name] = {};
  for (const [page, modes] of Object.entries(results)) {
    summary[name][page] = {};
    for (const [mode, text] of Object.entries(modes)) {
      writeFileSync(join(OUT, name, `${page}.${mode}.txt`), text);
      summary[name][page][mode] = { bytes: Buffer.byteLength(text), tokens: countTokens(text) };
    }
  }
}
writeFileSync(join(OUT, 'footprint.json'), JSON.stringify(summary, null, 2));

for (const mode of ['interactive', 'full']) {
  console.log(`\n${mode} snapshot, o200k tokens`);
  console.log(['page'.padEnd(8), ...only.map((t) => t.padStart(15))].join(''));
  const totals = Object.fromEntries(only.map((t) => [t, 0]));
  for (const page of PAGES.map((p) => p.name)) {
    const row = only.map((t) => {
      const n = summary[t][page][mode].tokens;
      totals[t] += n;
      return String(n).padStart(15);
    });
    console.log([page.padEnd(8), ...row].join(''));
  }
  console.log(['total'.padEnd(8), ...only.map((t) => String(totals[t]).padStart(15))].join(''));
}
