//! JSONL transcript parser — counts non-thinking and thinking text bytes,
//! and extracts the latest `message.usage` token totals.
//!
//! Ported from `parsers/rust/src/main.rs` and promoted to a library function
//! so both the CLI binary and `parsers/rust` (backwards-compat binary) can use it.
//!
//! ## Why two counters?
//!
//! Byte counting is the legacy approach: it works without any cooperation from
//! Claude Code, but file size accumulates across `/clear` and auto-compact
//! while the *actual* context window resets. CC writes a `message.usage` block
//! on every assistant turn that reflects the true window load (input +
//! cache_read + cache_creation + output tokens). When that block is present we
//! use it as the authoritative count; otherwise we fall back to byte counting.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// A single line from the Claude Code JSONL transcript.
#[derive(Deserialize)]
pub struct TranscriptLine {
    pub(crate) message: Option<Message>,
    #[serde(rename = "type", default)]
    pub(crate) entry_type: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Message {
    pub(crate) content: Option<Content>,
    #[serde(default)]
    pub(crate) usage: Option<Usage>,
}

/// Per-turn token usage as reported by Claude Code on assistant entries.
///
/// All four fields contribute to the live context window count.
#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Tokens in the user / system input portion of the request.
    #[serde(default)]
    pub input_tokens: u64,
    /// Tokens written to the prompt cache by this turn.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Tokens read from the prompt cache for this turn.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Tokens emitted by the model on this turn.
    #[serde(default)]
    pub output_tokens: u64,
}

impl Usage {
    /// Sum of all four token fields — the total live-window token count for
    /// the assistant turn that emitted this usage block.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

/// Content is either a plain string or an array of typed content blocks.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Content {
    Plain(String),
    Blocks(Vec<ContentBlock>),
}

/// A typed content block within a message.
#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "thinking")]
    Thinking { thinking: Option<String> },
    #[serde(rename = "text")]
    Text { text: Option<String> },
    #[serde(rename = "tool_use")]
    ToolUse { input: Option<serde_json::Value> },
    #[serde(rename = "tool_result")]
    ToolResult { content: Option<ToolResultContent> },
    /// Catch-all for block types we don't care about (e.g. "image").
    #[serde(other)]
    Unknown,
}

/// `tool_result` content: either a string or an array of text objects.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum ToolResultContent {
    Plain(String),
    Parts(Vec<ToolResultPart>),
}

#[derive(Deserialize)]
pub(crate) struct ToolResultPart {
    pub(crate) text: Option<String>,
}

/// Parse the JSONL transcript delta starting at `offset` bytes into the file.
///
/// Returns `(non_thinking_bytes, thinking_bytes)` accumulated over all new lines.
/// The first incomplete line at `offset` is always skipped (the monitor writes a
/// full line per call, but the seek may land mid-line if the offset was recorded
/// before the newline was flushed — skipping the first line is the safe choice,
/// matching the original shell implementation).
pub fn parse_transcript_delta(path: &Path, offset: u64) -> Result<(u64, u64)> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;

    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file_len <= offset {
        return Ok((0, 0));
    }

    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seek failed in {}", path.display()))?;

    let reader = BufReader::new(file);
    let mut total_nt: u64 = 0;
    let mut total_th: u64 = 0;
    let mut first_line = true;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if first_line {
            first_line = false;
            continue;
        }

        if line.is_empty() {
            continue;
        }

        let parsed: TranscriptLine = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let (nt, th) = count_text_bytes(&parsed);
        total_nt += nt;
        total_th += th;
    }

    Ok((total_nt, total_th))
}

/// Returns `(non_thinking_bytes, thinking_bytes)` for a single transcript line.
pub(crate) fn count_text_bytes(line: &TranscriptLine) -> (u64, u64) {
    let content = match line.message.as_ref().and_then(|m| m.content.as_ref()) {
        Some(c) => c,
        None => return (0, 0),
    };

    match content {
        Content::Plain(s) => (s.len() as u64, 0),
        Content::Blocks(blocks) => {
            let mut nt: u64 = 0;
            let mut th: u64 = 0;
            for block in blocks {
                match block {
                    ContentBlock::Thinking { thinking: Some(s) } => {
                        th += s.len() as u64;
                    }
                    ContentBlock::Text { text: Some(s) } => {
                        nt += s.len() as u64;
                    }
                    ContentBlock::ToolUse { input: Some(v) } => {
                        nt += v.to_string().len() as u64;
                    }
                    ContentBlock::ToolResult { content: Some(c) } => {
                        nt += count_tool_result_bytes(c);
                    }
                    ContentBlock::Thinking { thinking: None }
                    | ContentBlock::Text { text: None }
                    | ContentBlock::ToolUse { input: None }
                    | ContentBlock::ToolResult { content: None }
                    | ContentBlock::Unknown => {}
                }
            }
            (nt, th)
        }
    }
}

/// Scan the entire JSONL transcript and return the most recent
/// `message.usage` block found on an assistant entry.
///
/// Returns `None` when:
/// - the file does not exist or cannot be opened,
/// - no assistant entry with a `usage` block has been written yet (fresh
///   session before the first model turn).
///
/// We always scan from byte 0 because the latest assistant turn is whatever
/// has the greatest byte offset; we don't need to track a delta here, only
/// the most recent absolute reading.
pub fn parse_latest_usage(path: &Path) -> Option<Usage> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut latest: Option<Usage> = None;
    for line in reader.lines().map_while(Result::ok) {
        if line.is_empty() {
            continue;
        }
        let parsed: TranscriptLine = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let is_assistant = parsed
            .entry_type
            .as_deref()
            .map(|t| t == "assistant")
            .unwrap_or(true);
        if !is_assistant {
            continue;
        }

        if let Some(u) = parsed.message.as_ref().and_then(|m| m.usage) {
            // Skip empty usage records (all zeros): they appear on tool-result
            // continuation entries that don't represent a fresh model turn.
            if u.total() > 0 {
                latest = Some(u);
            }
        }
    }
    latest
}

pub(crate) fn count_tool_result_bytes(content: &ToolResultContent) -> u64 {
    match content {
        ToolResultContent::Plain(s) => s.len() as u64,
        ToolResultContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| p.text.as_ref())
            .map(|s| s.len() as u64)
            .sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_count(json: &str) -> (u64, u64) {
        let line: TranscriptLine = serde_json::from_str(json).unwrap();
        count_text_bytes(&line)
    }

    #[test]
    fn plain_string_content() {
        let json = r#"{"message":{"content":"hello world"}}"#;
        assert_eq!(parse_and_count(json), (11, 0));
    }

    #[test]
    fn text_block() {
        let json = r#"{"message":{"content":[{"type":"text","text":"abc"}]}}"#;
        assert_eq!(parse_and_count(json), (3, 0));
    }

    #[test]
    fn thinking_block() {
        let json = r#"{"message":{"content":[{"type":"thinking","thinking":"deep thoughts"}]}}"#;
        assert_eq!(parse_and_count(json), (0, 13));
    }

    #[test]
    fn mixed_blocks() {
        let json = r#"{"message":{"content":[
            {"type":"text","text":"visible"},
            {"type":"thinking","thinking":"hidden"},
            {"type":"text","text":"more"}
        ]}}"#;
        assert_eq!(parse_and_count(json), (11, 6));
    }

    #[test]
    fn tool_use_block() {
        let json = r#"{"message":{"content":[{"type":"tool_use","input":{"key":"val"}}]}}"#;
        let (nt, th) = parse_and_count(json);
        assert_eq!(nt, r#"{"key":"val"}"#.len() as u64);
        assert_eq!(th, 0);
    }

    #[test]
    fn tool_result_plain_string() {
        let json = r#"{"message":{"content":[{"type":"tool_result","content":"result text"}]}}"#;
        assert_eq!(parse_and_count(json), (11, 0));
    }

    #[test]
    fn tool_result_parts_array() {
        let json = r#"{"message":{"content":[{"type":"tool_result","content":[{"text":"part1"},{"text":"part2"}]}]}}"#;
        assert_eq!(parse_and_count(json), (10, 0));
    }

    #[test]
    fn no_message_field() {
        let json = r#"{"type":"system"}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }

    #[test]
    fn no_content_field() {
        let json = r#"{"message":{"role":"assistant"}}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }

    #[test]
    fn unknown_block_type_ignored() {
        let json = r#"{"message":{"content":[{"type":"image","source":"whatever"},{"type":"text","text":"hi"}]}}"#;
        assert_eq!(parse_and_count(json), (2, 0));
    }

    #[test]
    fn empty_blocks_array() {
        let json = r#"{"message":{"content":[]}}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }

    #[test]
    fn thinking_block_null_text() {
        let json = r#"{"message":{"content":[{"type":"thinking","thinking":null}]}}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }

    #[test]
    fn tool_result_empty_parts() {
        let json =
            r#"{"message":{"content":[{"type":"tool_result","content":[{"text":null},{}]}]}}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }

    // ── parse_latest_usage ───────────────────────────────────────────────────

    use std::io::Write as _;
    use tempfile::TempDir;

    fn write_jsonl(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("transcript.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn latest_usage_sums_all_four_fields() {
        let dir = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":300,"output_tokens":4}}}"#;
        let path = write_jsonl(&dir, &format!("{line}\n"));
        let u = parse_latest_usage(&path).expect("usage present");
        assert_eq!(u.total(), 334);
    }

    #[test]
    fn latest_usage_picks_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let earlier = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}"#;
        let later = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}"#;
        let path = write_jsonl(&dir, &format!("{earlier}\n{later}\n"));
        let u = parse_latest_usage(&path).expect("usage present");
        assert_eq!(u.total(), 500);
    }

    #[test]
    fn latest_usage_returns_none_when_no_usage_block() {
        let dir = tempfile::tempdir().unwrap();
        let content = "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n{\"type\":\"user\",\"message\":{\"content\":\"there\"}}\n";
        let path = write_jsonl(&dir, content);
        assert!(parse_latest_usage(&path).is_none());
    }

    #[test]
    fn latest_usage_returns_none_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.jsonl");
        assert!(parse_latest_usage(&path).is_none());
    }

    #[test]
    fn latest_usage_resets_after_clear_boundary() {
        // After /clear or auto-compact CC writes a fresh assistant turn whose
        // usage block reflects the *new* (smaller) window. The latest reading
        // returned here is the post-clear small one — what the throttle uses
        // to detect compaction.
        let dir = tempfile::tempdir().unwrap();
        let big = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":50000,"cache_creation_input_tokens":100000,"cache_read_input_tokens":600000,"output_tokens":8000}}}"#;
        let small = r#"{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":2000,"cache_creation_input_tokens":1000,"cache_read_input_tokens":500,"output_tokens":300}}}"#;
        let path = write_jsonl(&dir, &format!("{big}\n{small}\n"));
        let u = parse_latest_usage(&path).expect("usage present");
        assert_eq!(u.total(), 3800);
    }
}
