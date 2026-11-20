# Snapshot and Refs

Compact element references that reduce context usage dramatically for AI agents.

**Related**: [commands.md](commands.md) for full command reference, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [How Refs Work](#how-refs-work)
- [Snapshot Command](#the-snapshot-command)
- [Using Refs](#using-refs)
- [Ref Lifecycle](#ref-lifecycle)
- [Best Practices](#best-practices)
- [Ref Notation Details](#ref-notation-details)
- [Troubleshooting](#troubleshooting)

## How Refs Work

Traditional approach:
```
Full DOM/HTML → AI parses → CSS selector → Action (~3000-5000 tokens)
```

frob approach:
```
Compact snapshot → @refs assigned → Direct interaction (~200-400 tokens)
```

## The Snapshot Command

```bash
# Basic snapshot (shows page structure)
frob snapshot

# Interactive snapshot (-i flag) - RECOMMENDED
frob snapshot -i
```

Compact snapshots (`-c`) keep visible text, status text, and their direct ancestors. Empty sibling branches are removed. Interactive snapshots (`-i`) filter out noninteractive content. MCP snapshots default to interactive; set `interactive: false` to retain visible text and status text.

### Snapshot Output Format

A full snapshot (`snapshot`):

```
- navigation
  - link "Home" [ref=e1]
  - link [ref=e2] hint="img:cart"
    - image
- main
  - heading "Orders" [level=1, ref=e3]
  - paragraph
    - text "You have 2 open orders. See the"
    - link "help page" [ref=e5]
    - text "."
  - table
    - row
      - columnheader "Order" [ref=e7]
      - columnheader "Total" [ref=e8]
    - row
      - cell "Blue mug" [ref=e10]
      - cell "$12" [ref=e11]
      - cell [ref=e12] > button "Edit" [ref=e16]
  - textbox "Email" [ref=e6]
  - button "Subscribe" [ref=e4]
```

The same page with `snapshot -i`, which keeps only elements to act on, plus headings and landmarks:

```
- link "Home" [ref=e1]
- link [ref=e2] hint="img:cart"
- heading "Orders" [level=1, ref=e3]
- link "help page" [ref=e5]
- button "Edit" [ref=e16] hint="in:Blue mug $12"
- button "Edit" [ref=e17] hint="in:Red plate $9"
- textbox "Email" [ref=e6]
- button "Subscribe" [ref=e4]
```

A snapshot prints what an agent needs and leaves out what repeats or carries no meaning:

- Wrappers with no role of their own (unnamed `generic` elements, layout tables, labels, and inline formatting such as `strong` or `code`) give way to their content.
- Text that the page splits across inline elements is joined into one `text` line. A paragraph reads as text lines between its links.
- A node whose only content is one run of text shows that text as its name, and a name that only repeats the node's content is printed once.
- `-i` leaves out table cells and list items. Their refs still exist, so `get text @e10` works, and a full snapshot shows them.

### Hints for ambiguous controls

A control with an empty accessible name, or a name shared with another ref in the same snapshot, gets a hint read from the DOM after its ref. `in:` (the text of the surrounding row or list item) appears only in `-i` snapshots, since a full snapshot already prints that text. `id:`, `name:`, and `data-testid:` appear only when the value is made of words, such as `edit-btn`; generated values such as `mwEek` are skipped:

```
- link [ref=e1] hint="img:cart"
- button [ref=e2] hint="icon:icon-trash; in:Blue mug"
- button [ref=e3] hint="icon:fa-heart; in:Blue mug"
- button [ref=e4] hint="img:pencil; id:edit-btn; in:Blue mug"
- textbox "Email" [ref=e5] hint="name:login-email"
- textbox "Email" [ref=e6] hint="name:newsletter-email"
```

### Merged lines

A list item, table cell, or heading that only wraps one element whose name contains its own is printed on that element's line. Both refs work:

```
- listitem [level=1, ref=e3] > link "Twin bed $80 Castro Valley" [ref=e4]
```

Use `@e4` to act on the link and `@e3` for the list item. Set `FROB_SNAPSHOT_MERGE=0` to print them on separate lines.

### Collapsing long pages

`snapshot --collapse [n]` keeps the snapshot under `n` lines (default 150). When the page is longer, it folds its biggest sections, largest first, into one line that keeps the section's ref and says how many refs it holds:

```
- region "References" [ref=e406] (collapsed: 829 refs)
(14 sections collapsed to keep this under 150 lines. Expand one with `snapshot -i --collapse -s @<ref>`.)
```

Refs inside a folded section stay valid. To see a section, scope a snapshot to its ref with `-s @e406`, keeping `--collapse` to drill down further. A page within the budget prints unchanged. Flat pages, such as a long list with no sections, cannot be folded and print in full.

Hints are on by default in every snapshot mode and also appear in the `snapshot` text of `--json` output. A hint is not the accessible name, so `find role <role> --name` and stale-ref recovery ignore it: act on these elements by ref. Iframe refs never get a hint, and a control with nothing to read prints no hint.

## Using Refs

Once you have refs, interact directly:

```bash
# Click the "Sign In" button
frob click @e6

# Fill email input
frob fill @e10 "user@example.com"

# Fill password
frob fill @e11 "password123"

# Submit the form
frob click @e12
```

## Ref Lifecycle

Same-document updates preserve refs for surviving DOM elements. Replaced elements and page or iframe document replacements invalidate the corresponding refs. Invalidated IDs are never recycled within the browser session. JSON snapshot responses list refs that disappeared in `removedRefs`. Virtual accessibility nodes have snapshot-local refs.

A ref keeps pointing at its DOM node while that node exists. If the node was replaced, frob falls back to the one element with the same role and name, but only when the ref was unique in its snapshot. When duplicates make the match ambiguous, the action fails without acting. Run `snapshot` again and use a fresh ref.

```bash
# Get initial snapshot
frob snapshot -i
# - button "Next" [ref=e1]

# Click triggers page change
frob click @e1

# Re-snapshot after navigation to get new refs
frob snapshot -i
# - heading "Page 2" [level=1, ref=e2]  ← New element, new ref
```

## Best Practices

### 1. Always Snapshot Before Interacting

```bash
# CORRECT
frob open https://example.com
frob snapshot -i          # Get refs first
frob click @e1            # Use ref

# WRONG
frob open https://example.com
frob click @e1            # Ref doesn't exist yet!
```

### 2. Re-Snapshot After Navigation

```bash
frob click @e5            # Navigates to new page
frob snapshot -i          # Get new refs
frob click @e1            # Use new refs
```

### 3. Re-Snapshot After Dynamic Changes

```bash
frob click @e1            # Opens dropdown
frob snapshot -i          # See dropdown items
frob click @e7            # Select item
```

### 4. Snapshot Specific Regions

For complex pages, snapshot specific areas:

```bash
# Snapshot just the form
frob snapshot -s @e9
```

## Ref Notation Details

```
- checkbox "Remember me" [checked=true, ref=e12]
│ │        │             │
│ │        │             └─ State attributes, then the ref to act on (@e12)
│ │        └─ Accessible name: the label, text, or aria-label
│ └─ ARIA role
└─ One element per line; indentation shows nesting
```

### Common Patterns

```
- button "Submit" [ref=e10]                          # Button with text
- textbox "Email" [ref=e17]                          # Input, named by its label
- link "Link Text" [ref=e11]                         # Link
- combobox "Country" [expanded=false, ref=e19]: Norway   # Select; its value follows the colon
- checkbox "Remember me" [checked=true, ref=e12]     # Checked checkbox
- button "Annual" [pressed=true, ref=e17]            # Toggle button that is on (aria-pressed)
- radio "Yearly" [checked=false, ref=e14]            # Unchecked radio
- button "Delete" [disabled, ref=e16]                # Disabled control
- generic "Open menu" [ref=e15] clickable [cursor:pointer, onclick]   # Clickable element without a role
```

## Iframes

Presentational iframes use the same `Iframe` role for snapshots and stale-ref recovery.

Snapshots automatically detect and inline iframe content. When the main-frame snapshot runs, each `Iframe` node is resolved and its child accessibility tree is included directly beneath it in the output. Refs assigned to elements inside iframes carry frame context, so interactions like `click`, `fill`, and `type` work without manually switching frames.

```bash
frob snapshot -i
# - heading "Checkout" [level=1, ref=e1]
# - Iframe "payment-frame" [ref=e2]
#   - textbox "Card number" [ref=e3]
#   - textbox "Expiry" [ref=e4]
#   - button "Pay" [ref=e5]
# - button "Cancel" [ref=e6]

# Interact with iframe elements directly using their refs
frob fill @e3 "4111111111111111"
frob fill @e4 "12/28"
frob click @e5
```

**Key details:**
- Only one level of iframe nesting is expanded (iframes within iframes are not recursed)
- Cross-origin iframes that block accessibility tree access are silently skipped
- Empty iframes or iframes with no interactive content are omitted from the output
- To scope a snapshot to a single iframe, use `frame @ref` then `snapshot -i`

## Troubleshooting

### "Ref not found" Error

```bash
# Ref may have changed - re-snapshot
frob snapshot -i
```

### Element Not Visible in Snapshot

```bash
# Scroll down to reveal element
frob scroll down 1000
frob snapshot -i

# Or wait for dynamic content
frob wait 1000
frob snapshot -i
```

### Too Many Elements

```bash
# Snapshot specific container
frob snapshot @e5

# Or use get text for content-only extraction
frob get text @e5
```
