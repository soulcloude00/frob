# Common Slack Tasks & Patterns

Reference guide for common automations and data extraction patterns when interacting with Slack.

## Task: Check All Unread Messages

### Goal
Determine which channels and DMs have unread messages.

### Steps

1. **Connect to Slack**
   ```bash
   frob connect 9222
   ```

2. **Check Activity Tab**
   - Take snapshot: `frob snapshot -i`
   - Look for Activity tab ref (usually `@e14`)
   - Click: `frob click @e14`
   - Wait: `frob wait 1000`
   - If you see "You've read all the unreads", you have no unread messages
   - Screenshot: `frob screenshot activity.png`

3. **Check DMs**
   - Click DMs tab ref (usually `@e13`)
   - Look for "Unreads" toggle/badge
   - Count visible conversations with indicators

4. **Check Channels**
   - Look for "More unreads" button (usually in sidebar)
   - Click it to expand list of channels with unreads
   - Screenshot the expanded view
   - Parse channel names from snapshot

5. **Summary**
   - Activity + DMs + Channels = complete unread picture

### Evidence Capture
- Screenshot of Activity tab
- Screenshot of DMs
- Screenshot of expanded unreads sidebar

---

## Task: Find All Channels in Workspace

### Goal
Get a complete list of all channels you have access to.

### Steps

1. **Navigate to Channels section**
   ```bash
   frob connect 9222
   frob snapshot -i
   ```

2. **Look for "Channels" treeitem**
   - This is usually a collapsed section header
   - Click to expand if collapsed
   - Screenshot: `frob screenshot all-channels.png`

3. **Scroll through sidebar**
   ```bash
   # If the list is long, scroll within the sidebar
   frob scroll down 500 --selector ".p-sidebar"
   frob screenshot channels-page-2.png
   ```

4. **Parse snapshot for channel list**
   ```bash
   frob snapshot --json > channels.json
   # Search JSON for treeitem elements with level=2 under "Channels" section
   ```

### Evidence
- JSON snapshot with all channel refs
- Screenshots of channel list
- Count of total channels

---

## Task: Search for Messages Containing Keywords

### Goal
Find all messages/threads mentioning specific terms.

### Steps

1. **Open search**
   ```bash
   frob snapshot -i
   # Find Search button ref (usually @e5)
   frob click @e5
   frob wait 500
   ```

2. **Enter search term**
   ```bash
   # Identify search input ref from snapshot
   frob fill @e_search_input "your keyword"
   frob press Enter
   # Match the submitted query so a repeated search cannot reuse the old route.
   frob wait --fn "location.pathname.includes('/search') && new URL(location.href).searchParams.get('q') === 'your keyword'"
   ```

3. **Capture results**
   ```bash
   frob screenshot search-results.png
   frob snapshot -i > search-snapshot.txt
   ```

4. **Parse results**
   - Look for result items in snapshot
   - Extract message content, sender, channel, timestamp
   - Follow links to view full context

### Filters
Slack search supports filters:
- `in:channel-name` - Search in specific channel
- `from:@user` - Messages from specific user
- `before:2026-02-25` - Messages before date
- `after:2026-02-20` - Messages after date
- `has:file` - Messages with files
- `has:emoji` - Messages with reactions

Example search: `"bug report" in:engineering from:@alice after:2026-02-20`

---

## Task: Monitor a Specific Channel for Activity

### Goal
Watch a channel and capture new messages/engagement.

### Steps

1. **Navigate to channel**
   ```bash
   frob connect 9222
   frob snapshot -i
   # Find channel ref from sidebar
   frob click @e_channel_ref
   # Replace `engineering` with the clicked channel's name, or wait for its known URL:
   frob wait --fn "document.querySelector('[data-qa=\"channel_header\"]')?.innerText.includes('engineering')"
   ```

2. **Check channel info**
   - Screenshot channel details: `frob screenshot channel-header.png`
   - Look for member count, description, topic

3. **View messages**
   ```bash
   # Jump to recent/unread
   frob press j  # Jump to unread in Slack
   frob wait 500
   frob screenshot recent-messages.png
   ```

4. **Scroll to see more**
   ```bash
   frob scroll down 500
   frob screenshot more-messages.png
   ```

5. **Check threads**
   - Click on messages with thread indicators
   - View replies in thread view
   - Screenshot: `frob screenshot thread.png`

### Evidence
- Channel info screenshot
- Message history screenshots
- Thread examples

---

## Task: Extract User Information from a Conversation

### Goal
Find who said what, when, and in what context.

### Steps

1. **Navigate to relevant channel or DM**
   ```bash
   frob click @e_conversation_ref
   frob wait 1000
   ```

2. **Take snapshot with context**
   ```bash
   frob snapshot --json > conversation.json
   ```

3. **Find message blocks**
   - In JSON, look for document/listitem elements
   - These contain: user name (button), timestamp (link), message text, reactions

4. **Extract structured data**
   - User: Found in button element with username
   - Time: Found in link with timestamp
   - Content: Text content of message
   - Reactions: Buttons showing emoji counts

5. **Screenshot key messages**
   ```bash
   frob screenshot important-message.png
   frob screenshot --annotate annotated-message.png
   ```

---

## Task: Track Reactions to a Message

### Goal
See who reacted to a message and with what emoji.

### Steps

1. **Find message with reactions**
   ```bash
   frob snapshot -i
   # Look for "N reaction(s)" buttons in messages
   ```

2. **Click reaction button to expand**
   ```bash
   frob click @e_reaction_button
   frob wait 500
   ```

3. **Capture reaction details**
   ```bash
   frob screenshot reactions.png
   # You'll see emoji, count, and list of users who reacted
   ```

4. **Extract data**
   - Emoji used
   - Number of people who reacted
   - User names (if visible in popup)

---

## Task: Find and Review Pinned Messages

### Goal
See messages that have been pinned in a channel.

### Steps

1. **Open a channel**
   ```bash
   frob click @e_channel_ref
   frob wait 1000
   frob snapshot -i
   ```

2. **Click Pins tab**
   - In channel view, look for "Pins" tab ref (usually near Messages, Files tabs)
   - Click it: `frob click @e_pins_tab`
   - Wait: `frob wait 500`

3. **View pinned messages**
   ```bash
   frob screenshot pins.png
   frob snapshot -i > pins-snapshot.txt
   ```

4. **Review each pin**
   - Click pin to see context
   - Note who pinned it, when, and why
   - Screenshot: `frob screenshot pin-detail.png`

---

## Pattern: Extract Timestamp from Link

In Slack snapshot, message timestamps appear as links. Example:
```
- link "Feb 25th at 10:26:22 AM" [ref=e151]
  - /url: https://vercel.slack.com/archives/C0A5RTN0856/p1772036782543189
```

The URL contains the timestamp in the fragment (`p1772036782543189`). This is a Slack message ID that uniquely identifies the message.

---

## Pattern: Understanding Channel/Thread Structure

```
- treeitem "channel-name" [ref=e94] [level=2]
  - group: (contains channel metadata or sub-items)
```

- **level=1**: Section headers (External connections, Starred, Channels, etc.)
- **level=2**: Individual channels/items within sections
- **level=3+**: Nested sub-items (rare in sidebar)

---

## Common Ref Patterns (Session-Dependent)

These refs vary per session, but follow patterns:

| Element | Typical Ref Range | How to Find |
|---------|------------------|------------|
| Home tab | e10-e20 | `snapshot -i \| grep "Home"` |
| DMs tab | e10-e20 | `snapshot -i \| grep "DMs"` |
| Activity tab | e10-e20 | `snapshot -i \| grep "Activity"` |
| Search | e5-e10 | `snapshot -i \| grep "Search"` |
| More unreads | e20-e30 | `snapshot -i \| grep "More unreads"` |
| Channel refs | e30+ | `snapshot -i \| grep "treeitem"` |

**Always take a fresh snapshot** to find current refs for the current session.

---

## Debugging: Element Not Found

If you can't find an element:

1. **Check it's visible**
   ```bash
   # Is the element on screen or off-screen?
   frob screenshot current-state.png
   # Compare screenshot to what you expected
   ```

2. **Try expanding/scrolling**
   ```bash
   # Sidebar might need scrolling
   frob scroll down 300 --selector ".p-sidebar"
   frob snapshot -i
   ```

3. **Check current URL**
   ```bash
   frob get url
   # Verify you're in the right section
   ```

4. **Wait for page to load**
   ```bash
   frob wait --fn "document.querySelector('[role=main]')"
   frob snapshot -i
   ```
