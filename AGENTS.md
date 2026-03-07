# AGENTS.md — reinject

Context rot prevention plugin for Claude Code. Monitors transcript growth and enables just-in-time re-injection when hook-injected context drifts out of the model's recency zone.

## What This Repo Is

A Claude Code plugin with two auto-registered hooks and a consumer library for writing context-preserving hooks.

## Build & Test

```bash
bash tests/test-should-reinject.sh    # integration tests
claude --plugin-dir .                 # test as CC plugin
```

No build step for bash/jq. Optional Rust parser in `parsers/rust/` needs `cargo build --release`.

## Project Structure

```
.claude-plugin/plugin.json     # Plugin manifest
hooks/
  hooks.json                   # Auto-registered hooks (UserPromptSubmit + SessionStart compact)
  context-monitor.sh           # Monitor: parses JSONL, writes cumulative byte counts
  compact-reset.sh             # Reset: clears state after compaction
  lib/should-reinject.sh       # Consumer library: reads status, checks thresholds
skills/
  reinject/SKILL.md            # Skill: creates consumer hooks via /reinject:reinject
parsers/
  jq/extract-text-bytes.jq    # jq filter for JSONL text extraction
  rust/                        # Optional Rust parser (faster)
examples/supabase-context.sh   # Example consumer hook
tests/
  test-should-reinject.sh      # Integration tests
  fixtures/sample.jsonl        # Test data
docs/
  ASSUMPTIONS.md               # All assumptions with confidence levels
  PLAN.md                      # Architecture and design decisions
  RESEARCH.md                  # Academic research backing
```

## Conventions

- Shell: bash 4+, `set -euo pipefail` in scripts (not in libraries that get sourced)
- All hook scripts must be executable
- State files: `/tmp/claude-reinject-$PPID/` (per-session, auto-cleaned on reboot)
- Consumer hook names must be unique strings
- jq filter must handle malformed JSON lines via `try ... // empty`
- Tests: plain bash, `assert_eq` helper, exit 1 on failure

## Key Design Decisions

1. **Monitor + Consumer split** — monitor parses JSONL once per user message (UserPromptSubmit), consumers read the result on PreToolUse. N hooks = 1 parse, not N.
2. **UserPromptSubmit → PreToolUse ordering** — no race condition, UPS completes before tool calls.
3. **Text bytes ÷ 3.5, not tokenizer** — ±15% accuracy, 0.1-1.6ms. Thresholds already compensate.
4. **Per-hook injection tracking** — each consumer measures growth since ITS last injection.
5. **Thinking blocks separate** — step 3 (growth) uses non-thinking only; step 4 (position) uses all.

## How Consumer Hooks Work

See `CLAUDE.md` for full instructions or `examples/supabase-context.sh` for a working example.

Pattern: capture stdin → check relevance → set threshold → `source should-reinject.sh` → `if should_reinject "name"; then inject; reinject_record "name"; fi`
