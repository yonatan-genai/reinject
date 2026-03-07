# reinject

Context rot prevention for Claude Code. Monitors transcript growth and re-injects context when it drifts out of the model's recency zone.

## The Problem

Claude Code hooks can inject context at the right moment (e.g., Supabase credentials when you run `supabase` commands). But in long sessions:

1. Injected context drifts from the end of context (recency zone) toward the middle (dead zone)
2. The model's reasoning accuracy degrades for content in the dead zone ([Liu et al., 2023](https://arxiv.org/abs/2307.03172))
3. Auto-compaction summarizes old content, losing specific injected details
4. Without re-injection, your hooks become increasingly useless

## How It Works

**Two components:**

1. **Monitor** (`context-monitor.sh`) — auto-registered on `UserPromptSubmit`. Parses the JSONL transcript delta once per user message and writes cumulative text byte counts to a status file. This is the only component that touches the transcript.

2. **Consumer library** (`should-reinject.sh`) — sourced by your hooks. Reads the monitor's status file and compares against per-hook injection history. Pure arithmetic — no JSONL parsing.

**No race conditions:** UserPromptSubmit completes before the model generates tool calls, so by the time any PreToolUse consumer fires, the status file is already updated.

**Two triggers:**
- **Absolute growth** (step 3): enough new non-thinking content has accumulated since YOUR last injection
- **Dead zone detection** (step 4): your injection has drifted to the middle 15-85% of context with enough total context for position to matter

**Compaction handling:** SessionStart `compact` hook resets all state, so the next relevant tool use triggers a fresh injection.

## Installation

### As a Claude Code plugin

```bash
claude plugins add /path/to/reinject
```

This auto-registers the monitor (UserPromptSubmit) and compaction reset (SessionStart compact). You still write your own consumer hooks.

### Manual installation

Copy `hooks/` and `parsers/` somewhere stable, then add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [{
      "hooks": [{
        "type": "command",
        "command": "/path/to/hooks/context-monitor.sh"
      }]
    }],
    "SessionStart": [{
      "matcher": "compact",
      "hooks": [{
        "type": "command",
        "command": "/path/to/hooks/compact-reset.sh"
      }]
    }]
  }
}
```

## Writing a Consumer Hook

```bash
#!/bin/bash
INPUT=$(cat)  # MUST capture stdin first

# Your relevance check (matcher logic)
COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty')
if ! printf '%s' "$COMMAND" | grep -qi 'my-tool'; then
  exit 0
fi

# Set your criticality tier (optional, default Medium = 105KB)
REINJECT_GROWTH_BYTES=52000  # High tier

# Source the consumer library
source "/path/to/hooks/lib/should-reinject.sh"

# Check if re-injection is needed
if ! should_reinject "my-hook-name"; then
  exit 0
fi

# Inject your context
jq -n --arg ctx "Your context here" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    additionalContext: $ctx
  }
}'

# Record that you injected
reinject_record "my-hook-name"
exit 0
```

## Configuration

All via environment variables (set before sourcing the library):

| Variable | Default | Description |
|----------|---------|-------------|
| `REINJECT_GROWTH_BYTES` | `105000` | Step 3 threshold (non-thinking text bytes) |
| `REINJECT_RECENCY_THRESHOLD` | `85` | Upper dead zone boundary (%) |
| `REINJECT_PRIMACY_THRESHOLD` | `15` | Lower dead zone boundary (%) |
| `REINJECT_MIN_CONTEXT_BYTES` | `21000` | Min context for position check (~6K tokens) |
| `REINJECT_PARSER` | `jq` | Monitor parser: `jq` or path to Rust binary |

### Criticality Tiers

| Tier | Bytes | ~Tokens | Use Case |
|------|-------|---------|----------|
| High | 52,000 | 15K | Credentials, security rules |
| Medium | 105,000 | 30K | Workflow guides, conventions |
| Low | 175,000 | 50K | Nice-to-have reminders |

## Architecture

```
UserPromptSubmit                    PreToolUse (Bash)
       │                                   │
  context-monitor.sh              your-hook.sh (consumer)
       │                                   │
  Parse JSONL delta              source should-reinject.sh
       │                                   │
  Write status file ──────────> Read status file
  (cumulative bytes)            Compare vs own injection history
       │                                   │
  Done (before model            Inject if threshold exceeded
   generates tool calls)        Record injection
```

## Requirements

- `jq` 1.7+
- `bash` 4+
- Claude Code v2.1.9+ (PreToolUse `additionalContext` support)

## Docs

- [PLAN.md](docs/PLAN.md) — full architecture
- [ASSUMPTIONS.md](docs/ASSUMPTIONS.md) — what we're building on, with confidence levels
- [RESEARCH.md](docs/RESEARCH.md) — academic research backing the approach
