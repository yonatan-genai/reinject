use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

/// A single line from the Claude Code JSONL transcript.
#[derive(Deserialize)]
struct TranscriptLine {
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<Content>,
}

/// Content is either a plain string or an array of typed content blocks.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Plain(String),
    Blocks(Vec<ContentBlock>),
}

/// A typed content block within a message.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
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

/// tool_result content: either a string or an array of text objects.
#[derive(Deserialize)]
#[serde(untagged)]
enum ToolResultContent {
    Plain(String),
    Parts(Vec<ToolResultPart>),
}

#[derive(Deserialize)]
struct ToolResultPart {
    text: Option<String>,
}

/// Returns (non_thinking_bytes, thinking_bytes) for a transcript line.
fn count_text_bytes(line: &TranscriptLine) -> (u64, u64) {
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
                    _ => {}
                }
            }
            (nt, th)
        }
    }
}

fn count_tool_result_bytes(content: &ToolResultContent) -> u64 {
    match content {
        ToolResultContent::Plain(s) => s.len() as u64,
        ToolResultContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| p.text.as_ref())
            .map(|s| s.len() as u64)
            .sum(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: reinject-parser <transcript_path> <byte_offset>");
        std::process::exit(1);
    }

    let path = &args[1];
    let offset: u64 = args[2].parse().unwrap_or(0);

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => {
            println!("0 0");
            return;
        }
    };

    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file_len <= offset {
        println!("0 0");
        return;
    }

    if let Err(e) = file.seek(SeekFrom::Start(offset)) {
        eprintln!("seek failed: {e}");
        println!("0 0");
        return;
    }

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

    println!("{} {}", total_nt, total_th);
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
        // serde_json::Value::to_string() produces compact JSON
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
        let json = r#"{"message":{"content":[{"type":"tool_result","content":[{"text":null},{}]}]}}"#;
        assert_eq!(parse_and_count(json), (0, 0));
    }
}
