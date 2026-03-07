#!/bin/bash
# Example consumer: re-inject Supabase context when supabase commands are detected.
#
# The context-monitor.sh (UserPromptSubmit) has already parsed the JSONL
# and written cumulative byte counts. This hook just reads those counts
# and compares against its own injection history — pure arithmetic.
#
# Hook config (in your settings.json):
#   "PreToolUse": [{
#     "matcher": "Bash",
#     "hooks": [{ "type": "command", "command": "/path/to/supabase-context.sh" }]
#   }]

INPUT=$(cat)

# Only trigger on supabase CLI or API calls
COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if ! printf '%s' "$COMMAND" | grep -qiE '\bsupabase\b|supabase\.co'; then
  exit 0
fi

# High-criticality tier (52KB ≈ 15K tokens)
REINJECT_GROWTH_BYTES=52000

# Source the consumer library (adjust path to your installation)
source "$(dirname "$0")/../hooks/lib/should-reinject.sh"

if ! should_reinject "supabase-context"; then
  exit 0
fi

# Build context to inject
CONTEXT="Supabase credentials are in environment variables. Use \$SUPABASE_ACCESS_TOKEN for Management API."
GUIDE_FILE="${CLAUDE_PROJECT_DIR:-.}/.claude/rules/supabase-guide.md"
if [ -f "$GUIDE_FILE" ]; then
  CONTEXT=$(head -60 "$GUIDE_FILE")
fi

# Output additionalContext and record injection
jq -n --arg ctx "$CONTEXT" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    additionalContext: $ctx
  }
}'

# Record that we injected (updates per-hook state)
reinject_record "supabase-context"
exit 0
