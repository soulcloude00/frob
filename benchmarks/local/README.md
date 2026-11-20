# Local benchmarks

Benchmarks that run on one machine with no cloud account: frob against agent-browser and Playwright MCP, all driving the same Chrome for Testing binary.

```bash
cd benchmarks/local
pnpm install --ignore-workspace
```

## Context footprint

`footprint.mjs` opens eight real pages (Hacker News, Wikipedia, GitHub, MDN, an HTML form, BBC News, Amazon search, GOV.UK) in each tool and counts the tokens of a full and an interactive snapshot with the o200k tokenizer.

```bash
node footprint.mjs                                  # all three tools
node footprint.mjs --tools frob --frob ./path/to/frob
```

Raw snapshots and `footprint.json` land in `results/footprint/`. Live pages change, so compare tools from the same run.

## Agent tasks

`fixtures/` is a self-hosted site with 20 tasks built on UI patterns that separate browser tools: validation errors, wizards, consent and newsletter overlays, ARIA comboboxes, date pickers, iframes, shadow DOM, pagination, infinite scroll, menus, switches, dialogs, icon-only buttons, autocomplete, hidden-input radio cards, and client-side routes. Each task is judged by server state or an exact answer, never by a model. See `fixtures/README.md`; `node fixtures/selftest.mjs` proves every task is solvable and correctly judged.

The tasks run as the `frob-fixtures` suite of a fork of [web-agent-bench](https://github.com/nsosio/web-agent-bench), which drives one Claude model through each tool with identical prompts and blocks shortcuts such as in-page JavaScript and direct network calls. Run that suite with `--sequential`, since task state is shared per task.
