# Assumptions

Assumptions the context rot prevention system is built on. Each is categorized by confidence level and what breaks if it's wrong.

## Verified (from docs or source code)

### A1: `transcript_path` is available in hook stdin JSON
- **Source:** [Hooks reference](https://code.claude.com/docs/en/hooks) — common input fields table
- **Details:** Every hook event receives `transcript_path` as a field in the JSON input on stdin. NOT an environment variable.
- **If wrong:** Can't find the transcript file to parse. Fatal.

### A2: SessionStart fires with `source: "compact"` after compaction
- **Source:** Hooks reference — SessionStart matcher table
- **Details:** Matcher values include `startup`, `resume`, `clear`, `compact`. The `source` field in input JSON matches.
- **If wrong:** Can't detect compaction. Would need to fall back to PreCompact marker approach.

### A3: PreToolUse hooks support `additionalContext`
- **Source:** Hooks reference — PreToolUse decision control table. Added in v2.1.9.
- **Details:** `hookSpecificOutput.additionalContext` is "String added to Claude's context before the tool executes"
- **If wrong:** Can't inject context via PreToolUse hooks at all. Would need UserPromptSubmit.

### A4: `additionalContext` is added discretely (not shown in transcript)
- **Source:** Hooks reference — UserPromptSubmit section: "The additionalContext field is added more discretely" vs "Plain stdout is shown as hook output in the transcript"
- **If wrong:** Injected context would be visible in the conversation, which is cosmetic not functional.

## High Confidence (empirically validated but not explicitly documented)

### A5: `additionalContext` lands at recency position in context
- **Evidence:**
  - PreToolUse docs say "added to Claude's context before the tool executes" — implies current position
  - The feature was designed for "just-in-time context injection" (GitHub issue #15664)
  - Our existing supabase-context.sh hook uses this and Claude responds to the injected content
  - If it went to system prompt or middle of context, the feature would be useless for its stated purpose
- **If wrong:** The entire re-injection strategy collapses. Position-based triggers become meaningless. But this would also mean Anthropic shipped a broken feature, which is unlikely.
- **Mitigation:** None possible. This is a precondition of the hook injection mechanism having value.

### A6: CC auto-compacts at ~83.5% of 200K window (~167K tokens)
- **Evidence:** Observed empirically across multiple sessions. Matches community reports.
- **If wrong:** Thresholds might fire too early or too late. Not fatal — text bytes proxy still works, just with less optimal timing.

### A7: Thinking blocks don't count toward the 200K context/compaction limit
- **Evidence:** Anthropic docs state thinking tokens are separate from output tokens. Empirically, sessions with heavy thinking don't compact faster.
- **If wrong:** Our separation of thinking vs non-thinking text bytes would be unnecessary (but harmless).

## Medium Confidence (reasonable inference, not verified)

### A8: JSONL transcript format is stable enough
- **Details:** We depend on these field paths:
  - `.message.content` (string or array)
  - Content array items with `.type` = `"text"`, `"thinking"`, `"tool_use"`, `"tool_result"`
  - `.thinking` field on thinking blocks
  - `.text` field on text blocks
  - `.input` field on tool_use blocks
  - `.content` field on tool_result blocks (string or array)
  - `.isSidechain` and `.isApiErrorMessage` top-level flags
- **If wrong:** Parser silently undercounts text bytes. Thresholds fire later than expected. Degraded accuracy, not failure.
- **Mitigation:** Skip parse errors (resilience). Log parse failure count for debugging. Accept that the format could change.

### A9: Text bytes / token ratio of ~3.5 is conservative enough
- **Details:** Research shows ~4.0 for English prose, ~3.3-3.5 for code/JSON. We use 3.5 as divisor which overestimates tokens (triggers earlier = safe direction).
- **If wrong:** Thresholds would be off by the ratio error. At ±15% accuracy, this means triggering 15% too early or late.
- **Mitigation:** Configurable thresholds let users tune.

### A10: `tail -c +N` on a live JSONL file is safe
- **Details:** CC appends to the JSONL while hooks read it. A partial last line could cause a JSON parse error.
- **If wrong:** Last entry in delta gets skipped (undercount by one entry's worth of text bytes).
- **Mitigation:** We skip parse errors via `try ... // empty` in jq. We also skip the first line after seek (may be partial from mid-line offset). Maximum error is one JSONL entry — negligible.

## Low Confidence (design assumptions)

### A11: Dead zone boundaries at 15% and 85% apply to CC conversations
- **Details:** From Liu et al. (2023) "Lost in the Middle" — tested on specific NLP tasks (multi-document QA, KV retrieval). CC conversations are different (mixed tool results, code, text).
- **If wrong:** The dead zone might be narrower or wider. Position-based trigger fires at wrong thresholds.
- **Mitigation:** Boundaries are configurable (`REINJECT_PRIMACY_THRESHOLD`, `REINJECT_RECENCY_THRESHOLD`).

### A12: 6K tokens (~21KB text bytes) is the minimum context size where position matters
- **Details:** From Liu et al. (2023) — degradation starts appearing at 10-document context (~6K tokens). Below that, position effects are minimal.
- **If wrong:** We might skip position checks when they'd actually help, or run them when they're pointless.
- **Mitigation:** Configurable via `REINJECT_MIN_CONTEXT_BYTES`. The step 3 absolute growth threshold handles the common case anyway.
