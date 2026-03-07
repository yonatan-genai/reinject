# Context Rot Research Summary

Research findings that inform the context rot prevention strategy. Full paper details in the agent-finder-go repo at `dev/architecture/context-rot-research.md`.

## Key Papers

### Liu et al. (2023) — "Lost in the Middle"
- **Finding:** LLMs struggle with information in the middle of long contexts. Performance follows a U-curve: best at positions 0-15% (primacy) and 85-100% (recency), worst at 15-85% (dead zone).
- **Relevance:** Defines the dead zone boundaries (15%, 85%) used in step 4 position check.
- **Caveat:** Tested on multi-document QA and KV retrieval, not code generation. Dead zone boundaries may differ for CC conversations.

### Levy et al. (2024) — "Same Task, More Tokens"
- **Finding:** Reasoning accuracy degrades independently of retrieval. Even when the model retrieves the right information, it makes more reasoning errors as context grows. Degradation starts appearing around 3K-6K tokens.
- **Relevance:** Justifies the 21KB (~6K tokens) minimum context threshold for step 4. Below this, position effects are minimal.
- **Key insight:** Re-injection works because it moves content from dead zone to recency. The model can retrieve from recency AND reason correctly about it.

### Peysakhovich & Lerer (2023)
- **Finding:** Recency bias dominates at 16K+ tokens. Models disproportionately attend to recent content.
- **Relevance:** Confirms that re-injecting at recency position is the right strategy.

## Bytes/Token Ratios

### From web sources
- OpenAI tokenizer docs: "a token generally corresponds to ~4 characters of text for common English text" (~4 bytes/token for ASCII)
- Anthropic docs: similar guidance, ~3-4 characters per token
- Code and JSON tend toward ~3.3-3.5 bytes/token (more fragmented tokens)

### From empirical measurement (CC transcripts)
| Content Type | Bytes/Token | Notes |
|-------------|-------------|-------|
| User text (English) | ~4.0-4.2 | Prose-heavy |
| Assistant text | ~3.8-4.0 | Mix of prose and code |
| Thinking blocks | ~3.7-3.9 | Similar to assistant text |
| Tool use inputs | ~3.3-3.5 | JSON with code snippets |
| Tool results | ~3.5-3.8 | Varies widely (code vs prose) |
| **Weighted aggregate** | **~3.5-3.9** | Tool results dominate by volume |

### Decision: use ÷3.5
- Conservative lower bound → overestimates tokens → triggers re-injection earlier
- "Earlier than needed" is better than "too late" for context rot prevention
- At ÷3.5, a 105KB threshold ≈ 30K tokens. At actual ratio of ~3.8, it's ≈ 27.6K tokens. 8% error in the safe direction.

## Thresholds

### Step 3: Absolute growth (non-thinking text bytes)
| Tier | Text Bytes | Approx Tokens | Use Case |
|------|-----------|---------------|----------|
| High | 52 KB | ~15K | Credentials, security rules |
| Medium | 105 KB | ~30K | Workflow guides, conventions |
| Low | 175 KB | ~50K | Nice-to-have reminders |

### Step 4: Dead zone position
- Minimum context: 21 KB text bytes (~6K tokens)
- Dead zone: 15-85% proportional position
- Both thresholds configurable

### Compaction trigger
- CC auto-compacts at ~83.5% of 200K ≈ 167K tokens
- At ÷3.5 that's ~584KB text bytes
- Buffer before compaction: ~33K tokens (~115KB text bytes)
- High-tier (52KB) will fire ~2-3 times before compaction
- Medium-tier (105KB) will fire ~1-2 times before compaction

## Tokenizer Benchmarks (for reference)

These informed the decision to use text bytes proxy instead of actual tokenization.

| Approach | Cold Start | Warm | Memory | Notes |
|----------|-----------|------|--------|-------|
| bpe-openai (Rust) | 74-92ms | 15-40ms | ~50MB | Page cache dependent |
| kitoken (Rust) | ~24ms (.kit) / ~28ms (.tiktoken) | ~15ms | ~45MB | 2x slower encoding |
| tiktoken (Rust) | ~130ms | ~20ms | ~50MB | |
| Daemon (libc::poll) | N/A | 1-6ms IPC | 25MB RSS | Zero CPU idle |
| Text bytes proxy | 0.1-1.6ms | 0.1-1.6ms | negligible | No external deps |
| File size proxy | <0.1ms | <0.1ms | negligible | 13-39x variance, unusable |
