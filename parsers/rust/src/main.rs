use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

fn count_text_bytes(value: &Value) -> (u64, u64) {
    let content = match value.get("message").and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return (0, 0),
    };

    match content {
        Value::String(s) => (s.len() as u64, 0),
        Value::Array(items) => {
            let mut nt: u64 = 0;
            let mut th: u64 = 0;
            for item in items {
                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match item_type {
                    "thinking" => {
                        if let Some(s) = item.get("thinking").and_then(|v| v.as_str()) {
                            th += s.len() as u64;
                        }
                    }
                    "text" => {
                        if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
                            nt += s.len() as u64;
                        }
                    }
                    "tool_use" => {
                        if let Some(input) = item.get("input") {
                            nt += input.to_string().len() as u64;
                        }
                    }
                    "tool_result" => {
                        nt += count_tool_result_bytes(item);
                    }
                    _ => {}
                }
            }
            (nt, th)
        }
        _ => (0, 0),
    }
}

fn count_tool_result_bytes(item: &Value) -> u64 {
    match item.get("content") {
        Some(Value::String(s)) => s.len() as u64,
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|sub| sub.get("text").and_then(|v| v.as_str()))
            .map(|s| s.len() as u64)
            .sum(),
        _ => 0,
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

    file.seek(SeekFrom::Start(offset)).unwrap();
    let reader = BufReader::new(file);

    let mut total_nt: u64 = 0;
    let mut total_th: u64 = 0;
    let mut first_line = true;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Skip first line (may be partial from mid-offset seek)
        if first_line {
            first_line = false;
            continue;
        }

        if line.is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines
        };

        let (nt, th) = count_text_bytes(&value);
        total_nt += nt;
        total_th += th;
    }

    println!("{} {}", total_nt, total_th);
}
