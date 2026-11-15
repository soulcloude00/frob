#!/bin/bash
# Template: Authenticated Session Workflow
# Purpose: Login once, save state, reuse for subsequent runs
# Usage: ./authenticated-session.sh <login-url> [state-file]
#
# RECOMMENDED: Use the auth vault instead of this template:
#   echo "<pass>" | frob auth save myapp --url <login-url> --username <user> --password-stdin
#   frob auth login myapp
# The auth vault stores credentials securely and the LLM never sees passwords.
#
# Environment variables:
#   APP_USERNAME - Login username/email
#   APP_PASSWORD - Login password
#
# Two modes:
#   1. Discovery mode (default): Shows form structure so you can identify refs
#   2. Login mode: Performs actual login after you update the refs
#
# Setup steps:
#   1. Run once to see form structure (discovery mode)
#   2. Update refs in LOGIN FLOW section below
#   3. Set APP_USERNAME and APP_PASSWORD
#   4. Delete the DISCOVERY section

set -euo pipefail

LOGIN_URL="${1:?Usage: $0 <login-url> [state-file]}"
STATE_FILE="${2:-./auth-state.json}"

echo "Authentication workflow: $LOGIN_URL"

# ================================================================
# SAVED STATE: Skip login if valid saved state exists
# ================================================================
if [[ -f "$STATE_FILE" ]]; then
    echo "Loading saved state from $STATE_FILE..."
    if frob --state "$STATE_FILE" open "$LOGIN_URL" 2>/dev/null; then
        frob wait --load load

        CURRENT_URL=$(frob get url)
        if [[ "$CURRENT_URL" != *"login"* ]] && [[ "$CURRENT_URL" != *"signin"* ]]; then
            echo "Session restored successfully"
            frob snapshot -i
            exit 0
        fi
        echo "Session expired, performing fresh login..."
        frob close 2>/dev/null || true
    else
        echo "Failed to load state, re-authenticating..."
    fi
    rm -f "$STATE_FILE"
fi

# ================================================================
# DISCOVERY MODE: Shows form structure (delete after setup)
# ================================================================
echo "Opening login page..."
frob open "$LOGIN_URL"
frob wait --load domcontentloaded
frob wait --fn "(() => { const usable = (el) => { const style = getComputedStyle(el); return !el.disabled && !el.readOnly && style.display !== 'none' && style.visibility !== 'hidden' && el.getClientRects().length > 0; }; const username = Array.from(document.querySelectorAll('input[type=email], input[type=text], input[autocomplete=username], input[name*=email i], input[name*=user i], input[name*=login i]')).some(usable); const password = Array.from(document.querySelectorAll('input[type=password]')).some(usable); return username && password; })()"

echo ""
echo "Login form structure:"
echo "---"
frob snapshot -i
echo "---"
echo ""
echo "Next steps:"
echo "  1. Note the refs: username=@e?, password=@e?, submit=@e?"
echo "  2. Update the LOGIN FLOW section below with your refs"
echo "  3. Set: export APP_USERNAME='...' APP_PASSWORD='...'"
echo "  4. Delete this DISCOVERY MODE section"
echo ""
frob close
exit 0

# ================================================================
# LOGIN FLOW: Uncomment and customize after discovery
# ================================================================
# : "${APP_USERNAME:?Set APP_USERNAME environment variable}"
# : "${APP_PASSWORD:?Set APP_PASSWORD environment variable}"
#
# frob open "$LOGIN_URL"
# frob wait --load domcontentloaded
# frob wait --fn "(() => { const usable = (el) => { const style = getComputedStyle(el); return !el.disabled && !el.readOnly && style.display !== 'none' && style.visibility !== 'hidden' && el.getClientRects().length > 0; }; const username = Array.from(document.querySelectorAll('input[type=email], input[type=text], input[autocomplete=username], input[name*=email i], input[name*=user i], input[name*=login i]')).some(usable); const password = Array.from(document.querySelectorAll('input[type=password]')).some(usable); return username && password; })()"
# frob snapshot -i
#
# # Fill credentials (update refs to match your form)
# frob fill @e1 "$APP_USERNAME"
# frob fill @e2 "$APP_PASSWORD"
# frob click @e3
# Replace this with the app-specific post-login wait, for example:
# frob wait --url "**/dashboard"
# frob wait --text "Welcome"
#
# # Verify login succeeded
# FINAL_URL=$(frob get url)
# if [[ "$FINAL_URL" == *"login"* ]] || [[ "$FINAL_URL" == *"signin"* ]]; then
#     echo "Login failed - still on login page"
#     frob screenshot /tmp/login-failed.png
#     frob close
#     exit 1
# fi
#
# # Save state for future runs
# echo "Saving state to $STATE_FILE"
# frob state save "$STATE_FILE"
# echo "Login successful"
# frob snapshot -i
