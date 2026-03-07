---
name: reinject
description: Creates a Claude Code PreToolUse hook with built-in context rot prevention. Use when the user wants a hook that re-injects context when it drifts out of the model's recency zone. Triggers on requests like "create a context-preserving hook", "hook that re-injects", "make a hook for [tool] that survives compaction", "reinject hook for [tool]".
---

# Create a Context-Preserving Hook with Reinject

The user wants a PreToolUse hook that uses the reinject plugin to automatically re-inject its content when context rot is detected.

## Information to Gather

Ask the user for (if not already provided via $ARGUMENTS):

1. **What tool/command to match** — e.g., "supabase", "terraform", "kubectl", a regex pattern
2. **What context to inject** — inline text, or a file path to read from (e.g., `.claude/rules/terraform.md`)
3. **Criticality tier** — High (52KB/~15K tokens), Medium (105KB/~30K tokens, default), Low (175KB/~50K tokens)
4. **Hook name** — unique identifier, e.g., "terraform-context" (defaults to kebab-case of the tool name)
5. **Where to save** — e.g., `~/.claude/hooks/`, `.claude/hooks/`, or a custom path

## Template

Generate the hook script following this EXACT pattern — do not deviate:

```bash
#!/bin/bash
INPUT=$(cat)

COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if ! printf '%s' "$COMMAND" | grep -qiE '{{GREP_PATTERN}}'; then
  exit 0
fi

REINJECT_GROWTH_BYTES={{THRESHOLD}}

source "${CLAUDE_PLUGIN_ROOT}/hooks/lib/should-reinject.sh"

if ! should_reinject "{{HOOK_NAME}}"; then
  exit 0
fi

{{CONTEXT_LOADING_LOGIC}}

jq -n --arg ctx "$CONTEXT" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    additionalContext: $ctx
  }
}'

reinject_record "{{HOOK_NAME}}"
exit 0
```

## Context Loading Patterns

**Inline text:**
```bash
CONTEXT="Your rules here.
Can be multiline."
```

**From a file:**
```bash
GUIDE_FILE="${CLAUDE_PROJECT_DIR:-.}/.claude/rules/terraform.md"
if [ -f "$GUIDE_FILE" ]; then
  CONTEXT=$(head -80 "$GUIDE_FILE")
else
  CONTEXT="Fallback inline context"
fi
```

**From a command:**
```bash
CONTEXT=$(terraform workspace show 2>/dev/null || echo "unknown workspace")
```

## After Creating the Script

1. Make it executable: `chmod +x <path>`
2. Add it to the user's settings.json under `PreToolUse` with the appropriate matcher:

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{
        "type": "command",
        "command": "<absolute_path_to_script>"
      }]
    }]
  }
}
```

3. If the user has existing PreToolUse hooks for the same matcher, add the new hook to the existing hooks array — don't create a duplicate matcher group.

## Validation

- Verify `jq` is available: `which jq`
- Verify the plugin is installed: check for `reinject` in `claude plugins list` or that `${CLAUDE_PLUGIN_ROOT}` resolves
- If the plugin isn't installed, fall back to an absolute path for sourcing the library
