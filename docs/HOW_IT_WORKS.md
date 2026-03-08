# How reinject works

## The problem

Claude Code hooks let you inject rules and context when specific tools fire. "Always use slog for logging." "Run supabase with service role." These inject once at the start of a conversation and work great — for a while.

As the conversation grows, those early instructions drift into the middle of the context window — the "dead zone" where the model's attention is weakest. Claude stops following them. Your carefully crafted rules become invisible.

The naive fix is injecting on every tool call. That works for 5 commands. Run 50 and you've shoved 50 copies of the same rules into the context, burning tokens and actually making things worse — you're pushing real conversation content into the dead zone faster by flooding the window with redundant copies.

## What reinject does

Reinject is the middle ground between "inject once and pray" and "inject every time and drown." It tracks how much the conversation has grown since your last injection and re-injects only when the math says Claude is likely forgetting.

## The flow

**Monitor** — fires on every user message (`UserPromptSubmit`) and tool result (`PostToolUse`). Parses the JSONL transcript delta since last check, counts bytes of text (non-thinking and thinking separately), writes cumulative totals to a status file. That's it — just counting.

**Consumer** — fires on `PreToolUse`, inside your custom hooks. Before a tool runs, your hook calls `should_reinject("my-hook-name")`. The library reads the monitor's byte counts and compares against the counts from the last time *this specific hook* injected. Two triggers:

- **Growth threshold**: enough new text has accumulated since last injection. Configurable per hook — 52KB for critical rules (credentials, security), 105KB for medium (workflow conventions), 175KB for nice-to-have reminders.
- **Dead zone position**: the last injection landed between 15-85% of total context (where attention is weakest).

If either fires → re-inject. If neither → skip, don't waste tokens.

**Compaction reset** — when Claude Code compresses the conversation (`SessionStart compact`), all the byte counts become meaningless. Wipe the state, start fresh. Next relevant tool call triggers a new injection.

**Sub-agent skip** — both the monitor and consumer library detect sub-agents (via `agent_id` in hook input) and exit immediately. Sub-agents are short-lived — tracking their context growth is pointless and no consumer would ever read the output.

## In practice

Your supabase-context hook injects DB connection rules. First tool call → injects. Next 30K tokens of conversation → no injection needed, `should_reinject` returns false. Then the growth threshold fires → re-injects the rules so Claude remembers them. The rules stay fresh without spamming every single tool call.

## The math

No tokenizer needed. Text bytes ÷ 3.5 ≈ tokens (conservative estimate, ~±15% accuracy, sub-millisecond). The thresholds are calibrated in bytes:

| Criticality | Bytes | ~Tokens | When to use |
|-------------|-------|---------|-------------|
| High | 52,000 | ~15K | Credentials, security rules, auth patterns |
| Medium | 105,000 | ~30K | Workflow conventions, code style |
| Low | 175,000 | ~50K | Nice-to-have reminders |

Dead zone boundaries: 15% (primacy cutoff) to 85% (recency cutoff). Minimum 21KB (~6K tokens) of total context before position checks kick in — no point checking position in a short conversation.
