#!/bin/bash
# Template: Form Automation Workflow
# Purpose: Fill and submit web forms with validation
# Usage: ./form-automation.sh <form-url>
#
# This template demonstrates the snapshot-interact-verify pattern:
# 1. Navigate to form
# 2. Snapshot to get element refs
# 3. Fill fields using refs
# 4. Submit and verify result
#
# Customize: Update the refs (@e1, @e2, etc.) based on your form's snapshot output

set -euo pipefail

FORM_URL="${1:?Usage: $0 <form-url>}"

echo "Form automation: $FORM_URL"

# Step 1: Navigate to form
frob open "$FORM_URL"
frob wait --load domcontentloaded
frob wait --fn "Array.from(document.querySelectorAll('input:not([type=hidden]), textarea, select')).some((el) => { const style = getComputedStyle(el); return !el.disabled && !el.readOnly && style.display !== 'none' && style.visibility !== 'hidden' && el.getClientRects().length > 0; })"

# Step 2: Snapshot to discover form elements
echo ""
echo "Form structure:"
frob snapshot -i

# Step 3: Fill form fields (customize these refs based on snapshot output)
#
# Common field types:
#   frob fill @e1 "John Doe"           # Text input
#   frob fill @e2 "user@example.com"   # Email input
#   frob fill @e3 "SecureP@ss123"      # Password input
#   frob select @e4 "Option Value"     # Dropdown
#   frob check @e5                     # Checkbox
#   frob click @e6                     # Radio button
#   frob fill @e7 "Multi-line text"   # Textarea
#   frob upload @e8 /path/to/file.pdf # File upload
#
# Uncomment and modify:
# frob fill @e1 "Test User"
# frob fill @e2 "test@example.com"
# frob click @e3  # Submit button

# Step 4: Wait for submission
# frob wait --url "**/success"  # Or wait for redirect
# frob wait --text "Success"     # Or wait for confirmation text

# Step 5: Verify result
echo ""
echo "Result:"
frob get url
frob snapshot -i

# Optional: Capture evidence
frob screenshot /tmp/form-result.png
echo "Screenshot saved: /tmp/form-result.png"

# Cleanup
frob close
echo "Done"
