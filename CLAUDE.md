# Context Rot Prevention Plugin

This plugin is installed and active. It auto-monitors transcript growth via UserPromptSubmit and resets state on compaction via SessionStart compact.

## Writing Consumer Hooks

When the user asks you to create a hook that should survive context rot (or "preserve context," "re-inject," "context-aware hook"), create a PreToolUse hook following this exact pattern:

```bash
#!/bin/bash
INPUT=$(cat)  # MUST be first — stdin is the hook JSON, can only be read once

# Relevance check
COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if ! printf '%s' "$COMMAND" | grep -qiE 'YOUR_PATTERN'; then
  exit 0
fi

# Threshold: High=52000, Medium=105000 (default), Low=175000
REINJECT_GROWTH_BYTES=52000

source "${CLAUDE_PLUGIN_ROOT}/hooks/lib/should-reinject.sh"

if ! should_reinject "UNIQUE_HOOK_NAME"; then
  exit 0
fi

CONTEXT="Your context content here"
jq -n --arg ctx "$CONTEXT" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    additionalContext: $ctx
  }
}'

reinject_record "UNIQUE_HOOK_NAME"
exit 0
```

## Rules

1. `INPUT=$(cat)` MUST be the first line — stdin can only be read once
2. Set `REINJECT_GROWTH_BYTES` BEFORE sourcing the library
3. Call `reinject_record` AFTER outputting the JSON — records current byte counts
4. Hook name must be unique per consumer
5. Register consumer hooks as PreToolUse with appropriate tool matcher in settings.json
6. Source the library from `${CLAUDE_PLUGIN_ROOT}/hooks/lib/should-reinject.sh`

## Tiers

| Tier | REINJECT_GROWTH_BYTES | ~Tokens | Use For |
|------|----------------------|---------|---------|
| High | 52000 | 15K | Credentials, security rules, auth patterns |
| Medium | 105000 | 30K | Workflow guides, coding conventions |
| Low | 175000 | 50K | Nice-to-have reminders, style preferences |
