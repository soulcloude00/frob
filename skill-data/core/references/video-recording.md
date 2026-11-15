# Video Recording

Capture browser automation as video for debugging, documentation, or verification.

**Related**: [commands.md](commands.md) for full command reference, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Requirements](#requirements)
- [Basic Recording](#basic-recording)
- [Recording Commands](#recording-commands)
- [Frame Rate](#frame-rate)
- [Visible Cursor](#visible-cursor)
- [High-Density Video](#high-density-video)
- [Contact Sheets](#contact-sheets)
- [Use Cases](#use-cases)
- [Best Practices](#best-practices)
- [Output Format](#output-format)
- [Limitations](#limitations)

## Requirements

Recording pipes frames into `ffmpeg`, which must be on `PATH` with the `libvpx` and `libx264` encoders. Install it with `brew install ffmpeg` (macOS) or `sudo apt install ffmpeg` (Debian/Ubuntu); `frob doctor` reports it under "Recording". Nothing else in frob needs ffmpeg.

Supported formats are `.webm` (VP8 via libvpx) and `.mp4` (H.264 via libx264). Other extensions are handed to ffmpeg as-is with H.264 video. A path with no extension is rejected before recording starts.

## Basic Recording

`record start` records the current active page as-is. Without a URL it attaches to the tab you already have open (no navigation, no new tab, page state and hydration intact). With a URL it navigates the active tab there first.

```bash
# Launch the browser, then start recording
frob open https://example.com
frob record start ./demo.webm

# Perform actions
frob snapshot -i
frob click @e1
frob fill @e2 "test input"

# Stop and save
frob record stop
```

## Recording Commands

```bash
# Launch a session first
frob open

# Start recording to file (30 fps)
frob record start ./output.webm

# Start recording at a specific rate (1-60)
frob record start ./output.webm --fps 60

# Stop current recording
frob record stop

# Restart with new file (stops current + starts new)
frob record restart ./take2.webm --fps 60

# Navigate the active tab, then record
frob record start ./output.webm https://example.com/checkout

# Record in a separate tab: open it first, then start recording
frob tab new https://example.com
frob record start ./output.webm
```

## Frame Rate

Recording captures 30 fps by default, so scrolling, hover states, and CSS transitions read as motion instead of a slideshow. `--fps` takes any rate from 1 to 60.

| Rate | Use it for |
| --- | --- |
| 60 | Short, motion-heavy takes: drag interactions, animation, scroll polish work |
| 30 (default) | Flows, CI evidence, walkthroughs |
| 1-15 | Long sessions where the video is a timeline, not a motion study |

```bash
# Animation review
frob record start ./transition.webm --fps 60
frob click @e1
frob wait 1500
frob record stop

# Hour-long soak run
frob record start ./soak.webm --fps 5
```

The video uses the requested frame rate and holds the latest Chrome frame between repaints. `record stop --json` reports `frames` (written) and `capturedFrames` (distinct frames from Chrome).

## Visible Cursor

Chrome's screencast does not include the native pointer. Pass `--cursor` to add an animated pointer and click ripple rendered with the page, keeping drags synchronized in every captured frame. The temporary overlay is inert, hidden from accessibility snapshots, and removed when recording stops. Screenshots taken during the recording include it.

```bash
frob record start ./walkthrough.webm --cursor
```

## High-Density Video

Frames come from Chrome's screencast, which captures at the window's real device scale. When the page's scale is emulated with `set viewport <w> <h> <scale>`, recording switches to full-density screenshots, at up to the recording's frame rate (a busy page can capture fewer frames); `record stop` reports `"capture": "screenshots"` and `deviceScaleFactor`. To keep the screencast at full density instead, launch with a real device scale:

```bash
frob --args "--force-device-scale-factor=2" open https://example.com
frob record start ./sharp.webm
```

## Contact Sheets

Pass `--contact-sheet` to create a timestamped summary with highlighted changes.

```bash
frob record start ./checkout.webm --contact-sheet

# Select frames when at least 2% of the image changes
frob record start ./checkout.webm --contact-sheet-threshold 0.02
```

The threshold accepts values from `0` to `1` and defaults to `0.05`. Passing `--contact-sheet-threshold` implies `--contact-sheet`. At most 100 frames are included.

## Use Cases

### Debugging Failed Automation

```bash
#!/bin/bash
# Record automation for debugging

# Run your automation
frob open https://app.example.com
frob record start ./debug-$(date +%Y%m%d-%H%M%S).webm
frob snapshot -i
frob click @e1 || {
    echo "Click failed - check recording"
    frob record stop
    exit 1
}

frob record stop
```

### Documentation Generation

```bash
#!/bin/bash
# Record workflow for documentation

frob open https://app.example.com/login
frob record start ./docs/how-to-login.webm
frob wait 1000  # Pause for visibility

frob snapshot -i
frob fill @e1 "demo@example.com"
frob wait 500

frob fill @e2 "password"
frob wait 500

frob click @e3
frob wait --url "**/dashboard"
frob wait 1000  # Show result

frob record stop
```

### CI/CD Test Evidence

```bash
#!/bin/bash
# Record E2E test runs for CI artifacts

TEST_NAME="${1:-e2e-test}"
RECORDING_DIR="./test-recordings"
mkdir -p "$RECORDING_DIR"

frob open
frob record start "$RECORDING_DIR/$TEST_NAME-$(date +%s).webm"

# Run test
if run_e2e_test; then
    echo "Test passed"
else
    echo "Test failed - recording saved"
fi

frob record stop
```

## Best Practices

### 1. Add Pauses for Clarity

```bash
# Slow down for human viewing
frob click @e1
frob wait 500  # Let viewer see result
```

### 2. Use Descriptive Filenames

```bash
# Include context in filename
frob record start ./recordings/login-flow-2024-01-15.webm
frob record start ./recordings/checkout-test-run-42.webm
```

### 3. Handle Recording in Error Cases

```bash
#!/bin/bash
set -e

cleanup() {
    frob record stop 2>/dev/null || true
    frob close 2>/dev/null || true
}
trap cleanup EXIT

frob open
frob record start ./automation.webm
# ... automation steps ...
```

### 4. Combine with Screenshots

```bash
# Record video AND capture key frames
frob open https://example.com
frob record start ./flow.webm
frob screenshot ./screenshots/step1-homepage.png

frob click @e1
frob screenshot ./screenshots/step2-after-click.png

frob record stop
```

## Output Format

- Format follows the extension: `.webm` (VP8 via libvpx) or `.mp4` (H.264 via libx264); other extensions get H.264 in that container
- Default frame rate: 30 fps (`--fps` accepts 1 to 60)
- Optional contact sheet: timestamped PNG with changed-region highlights
- Compatible with all modern browsers and video players
- Compressed but high quality

## Limitations

- Recording adds slight overhead to automation, and higher frame rates add more
- Large recordings can consume significant disk space
- Distinct frames per second are bounded by how often the page repaints, so a page rendering below 60 fps records below it too
- Some headless environments may have codec limitations; an ffmpeg built without libvpx or libx264 cannot write the matching format
