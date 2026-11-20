# Browser-agent fixtures

A self-hosted, deterministic fixture site with 20 tasks for comparing browser-automation tools (frob, agent-browser, Playwright MCP) driven by an LLM agent. Everything is inline HTML/CSS/JS served by a zero-dependency Node 22 server; nothing touches the network.

## Run

```bash
node server.mjs              # http://127.0.0.1:4517
node server.mjs --port 4600
node selftest.mjs            # starts its own server on 4518 and drives every task with the frob CLI
node selftest.mjs --only kanban,datepicker
```

Endpoints:

- `/t/<task-id>/...` task pages (start at `start_path` in `tasks.json`)
- `GET /__state/<task-id>` returns `{ "pass": boolean, "detail": string }`, judged from server-side state recorded by the page's submissions
- `POST /__reset` and `POST /__reset/<task-id>` clear state (reset before each run)
- `GET /__health` returns 200

`tasks.json` holds `{ id, start_path, goal, check }`. `check.type` is `state` (call `/__state/<id>` after the agent finishes) or `answer` (match `check.pattern` as a case-insensitive JS regex against the agent's final reply). Answer tasks always report `pass:false` from `/__state`.

The fake "today" for date tasks is Tuesday, March 10, 2026, shown on the page. All generated data uses fixed seeds.

`selftest.mjs` uses in-page JS through `frob eval` to drive the real UI and submission endpoints. For each state task it checks that state starts at `pass:false`, that a plausible wrong attempt stays `pass:false`, and that the right attempt reaches `pass:true`. For answer tasks it extracts the answer from the page, checks the pattern matches it, and checks it rejects near-miss replies. Set `FROB_BIN` to use another frob binary.

## Tasks

| id | UI challenge |
| --- | --- |
| login-validation | Client-side validation plus a server error on the first correct attempt that reveals a required Terms checkbox |
| checkout-wizard | Three-step wizard (shipping, payment, review) with hidden steps and a required agreement checkbox |
| cookie-consent | Modal consent dialog makes the page inert until dismissed; billing toggle uses `aria-pressed` buttons |
| newsletter-popup | Modal popup appears 2s after load and blocks submission; subscribing to it fails the task |
| combobox | Div-based ARIA combobox with typeahead; typed text alone is not a selection; near-miss options (New Caledonia) |
| datepicker | Readonly date field, calendar dialog two months ahead, weekend and past dates disabled, `role=radio` time slots loaded after picking |
| iframe-form | Billing form inside a same-origin iframe next to a decoy profile email field |
| shadow-dom | Gift card input and button inside nested open shadow roots; a light-DOM promo field is a decoy |
| table-pagination | Sortable, paginated table (only 10 rows in the DOM); answer about page 3, row 4 after sorting by salary descending |
| product-filter | Filters, sort, "Show more", identical "Add to cart" buttons; the cart must hold exactly the right item |
| infinite-scroll | Feed loads 10 posts at a time on scroll; target post is in batch 5 and the same author has decoy posts |
| kanban | Identically named "Card actions" menu buttons with a nested "Move to" submenu; similar card titles |
| settings-toggles | `role=switch` toggles, duplicate "Mentions" and "Replies" labels in two sections, Save disabled until dirty |
| delete-dialog | AlertDialog that requires typing the key name; `staging-ci` vs `staging-ci-legacy` |
| tabs-accordion | Needed fields hidden behind a tab and a collapsed accordion panel with its own Save button |
| icon-buttons | Icon-only "Edit" buttons identical on every row, unlabelled destructive icon buttons, Jordan Lee vs Jordan Leigh |
| autocomplete-search | Debounced server-backed suggestions limited to 5; Enter without a highlighted suggestion does nothing |
| radio-cards | Visually hidden radio inputs inside card labels; Priority support is disabled until a paid plan is chosen |
| spa-route | pushState navigation with delayed rendering, an Open/Paid filter, and a decoy invoice (INV-2430) with the same customer and amount |
| split-text-answer | Order number split across inline spans, plus a table sum restricted to one warehouse |
