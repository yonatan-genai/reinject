# Context Rot Prevention — Architecture & Plan

A Claude Code hook plugin that detects when injected context has drifted out of the model's recency zone and re-injects it to prevent context rot.

## Problem

Claude Code's context window is finite (~200K tokens). As a session progresses:
1. Previously injected context drifts from recency (end) toward the dead zone (middle 15-85%)
2. Reasoning accuracy degrades for content in the dead zone (Liu et al., 2023; Levy et al., 2024)
3. Auto-compaction (~167K tokens) summarizes old content, losing specific injected details
4. Without re-injection, hooks inject context once and it becomes increasingly useless

## Solution

A library (`should-reinject.sh`) that hooks call to decide whether to re-inject their context. Uses a text bytes proxy (no tokenizer needed) to approximate token growth and positional drift.

## Architecture

### State Files

State stored in `/tmp/claude-reinject-{session_id}/`:

**Monitor files** (written by `context-monitor.sh`):
- `monitor-status` — 2 lines: cumulative non-thinking bytes, cumulative thinking bytes
- `monitor-offset` — single value: byte offset into transcript JSONL

**Consumer files** (per hook name, e.g. `supabase-context`):
- 2 lines: non-thinking bytes and thinking bytes at time of last injection

### Flow (per hook invocation)

```
Hook fires (PreToolUse/UserPromptSubmit/PostToolUse/SessionStart)
  │
  ├─ Parse transcript_path from stdin JSON
  │
  ├─ Step 1: First run? → INJECT (no prior state)
  │
  ├─ Step 2: Compaction reset? → INJECT
  │   (SessionStart compact clears state; next hook call sees no state = step 1)
  │
  ├─ Step 3: Absolute growth check
  │   Read JSONL from saved offset → parse new lines → sum text bytes
  │   Separate: non-thinking bytes (for threshold) and thinking bytes (for position)
  │   If delta_non_thinking_bytes > THRESHOLD → INJECT
  │   Thresholds (in non-thinking text bytes):
  │     High:   52 KB  (~15K tokens ÷ 3.5)
  │     Medium: 105 KB (~30K tokens ÷ 3.5)
  │     Low:    175 KB (~50K tokens ÷ 3.5)
  │
  ├─ Step 4: Dead zone position check
  │   total_text = saved_text_bytes + delta_text_bytes (ALL text, including thinking)
  │   If total_text > 21 KB (~6K tokens, minimum for position effects):
  │     position = saved_text_bytes / total_text
  │     If 15% < position < 85% → INJECT (in dead zone)
  │
  └─ No trigger → SKIP (return 1)
```

### Why text bytes proxy?

| Approach | Latency | Accuracy | Dependencies |
|----------|---------|----------|-------------|
| Text bytes ÷ 3.5 | 0.1-1.6ms | ±15% | jq (or Rust binary) |
| Tokenizer (bpe-openai, cold) | 74-92ms | exact | Rust binary, page cache dependent |
| Tokenizer (daemon, warm) | 1-6ms | exact | Background process, 25MB RAM |
| File size proxy | <0.1ms | ±200% | none |

Text bytes proxy wins: fast enough for per-tool-use, accurate enough for threshold decisions, no external dependencies beyond jq.

### Bytes/token ratio

- English prose: ~4.0 bytes/token
- Code/JSON: ~3.3-3.5 bytes/token
- Mixed CC transcripts: ~3.5-3.9 bytes/token
- We use ÷3.5 (conservative lower bound) → overestimates tokens → triggers earlier → safe direction

### Thinking blocks: dual counting

Thinking blocks present a challenge:
- They DO occupy attention/position (model sees them during generation)
- They DON'T count toward the 200K compaction limit
- For step 3 (growth threshold → "how close to compaction?"): use **non-thinking** bytes only
- For step 4 (position → "where is my injection in the context?"): use **all** text bytes including thinking

### Compaction handling

SessionStart with matcher `compact` fires after auto/manual compaction. The hook:
1. Deletes all state files in `/tmp/claude-reinject-{session_id}/`
2. This means next hook call hits step 1 (first run) → injects

This replaces the old PreCompact marker + timestamp comparison approach. Simpler, fewer moving parts.

### Parser: jq vs Rust

Configurable via `REINJECT_PARSER` env var:
- **`jq`** (default): single jq pipeline, ~0.5-2ms for typical deltas. Requires jq 1.7+ for `utf8bytelength` (falls back to `length` on older versions).
- **Path to Rust binary**: for users who want <0.5ms. Needs `cargo install` or prebuilt binary.

Both parsers output: `<non_thinking_bytes> <thinking_bytes> <lines_parsed> <lines_errored>`

### Transcript path discovery

The `transcript_path` is provided in the hook's JSON stdin input (common field for all hook events). The hook parses it from stdin on every invocation. No env var derivation needed.

### JSONL delta parsing

1. `tail -c +$((offset + 1))` to seek to saved position
2. `tail -n +2` to skip potentially partial first line
3. Parse each line as JSON, extract text content
4. Sum bytes separately for thinking vs non-thinking content
5. `try ... // empty` in jq to skip malformed lines (concurrent write resilience)

### Hook types this supports

| Hook Event | Component | Matcher | Use Case |
|-----------|-----------|---------|----------|
| UserPromptSubmit | Monitor | N/A | Catches user message + previous turn's tail |
| PostToolUse | Monitor | N/A | Catches each tool result |
| PreToolUse | Consumer | Tool-specific (e.g., `Bash`) | Re-inject when relevant tool is used |
| UserPromptSubmit | Consumer | N/A | Re-inject on keyword match in prompt |
| SessionStart | Reset | `compact` | Reset state after compaction |

## File Structure

```
claude-context-hooks/
├── hooks/
│   ├── lib/
│   │   └── should-reinject.sh      # Core library
│   └── hooks.json                  # Plugin hook definitions
├── parsers/
│   ├── jq/
│   │   └── extract-text-bytes.jq   # jq filter for JSONL parsing
│   └── rust/
│       ├── Cargo.toml
│       └── src/main.rs             # Optional Rust parser
├── docs/
│   ├── ASSUMPTIONS.md              # All assumptions with confidence levels
│   ├── PLAN.md                     # This file
│   └── RESEARCH.md                 # Context rot research summary
├── tests/
│   ├── test-should-reinject.sh     # Integration tests
│   └── fixtures/                   # Test JSONL files
├── examples/
│   └── supabase-context.sh         # Example consumer hook
└── README.md
```

## Open Questions

1. **Plugin distribution format** — Claude Code plugins have a specific structure (`hooks/hooks.json`). Need to verify the plugin spec supports library files that consumer hooks can source.

2. ~~**PPID stability**~~ **Resolved.** Hooks now use `session_id` from JSON stdin (stable across the session, shared by orchestrator and sub-agents). Falls back to `$PPID` for backwards compat with non-CC callers.

3. **Multi-session edge case** — Two simultaneous sessions in the same project would have different PPIDs and different state dirs. Not a problem.

4. **jq version compatibility** — `utf8bytelength` requires jq 1.7+. macOS Homebrew ships 1.7.1 but system jq might be older. Need graceful fallback to `length` (codepoint count, close enough for ASCII-heavy content).
