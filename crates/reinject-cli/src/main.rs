//! `reinject` — single binary replacing all reinject shell scripts.
//!
//! ## Subcommands
//!
//! ```text
//! reinject monitor                      # context-monitor.sh
//! reinject check   <hook-name>          # should_reinject() → exit 0/1
//! reinject record  <hook-name>          # reinject_record()
//! reinject output  <hook-event> <ctx>   # reinject_output()
//! reinject reset                        # compact-reset.sh
//! reinject parse   <path> <offset>      # reinject-parser binary
//! ```
//!
//! All subcommands that need session context read a JSON object from stdin
//! with at least `session_id` and optionally `agent_id` and `transcript_path`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{bail, Context as _, Result};
use reinject_core::{
    hook_output, parse_transcript_delta, record, reset_state, should_reinject, state_dir,
    update_monitor, ThrottleConfig, ThrottleDecision,
};
use serde::Deserialize;

fn main() {
    if let Err(e) = run() {
        eprintln!("reinject: {e:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let subcmd = args.get(1).map(String::as_str).unwrap_or("");

    match subcmd {
        "monitor" => cmd_monitor(),
        "check" => {
            let hook_name = require_arg(&args, 2, "check <hook-name>")?;
            cmd_check(hook_name)
        }
        "record" => {
            let hook_name = require_arg(&args, 2, "record <hook-name>")?;
            cmd_record(hook_name)
        }
        "output" => {
            let hook_event = require_arg(&args, 2, "output <hook-event> <context>")?;
            let context = require_arg(&args, 3, "output <hook-event> <context>")?;
            let sys_msg = args.get(4).map(String::as_str);
            cmd_output(hook_event, context, sys_msg)
        }
        "reset" => cmd_reset(),
        "parse" => {
            let path_str = require_arg(&args, 2, "parse <transcript-path> <offset>")?;
            let offset_str = require_arg(&args, 3, "parse <transcript-path> <offset>")?;
            let offset: u64 = offset_str
                .parse()
                .with_context(|| format!("invalid offset: {offset_str}"))?;
            cmd_parse(Path::new(path_str), offset)
        }
        other => {
            eprintln!("Usage: reinject <monitor|check|record|output|reset|parse> [args...]");
            eprintln!();
            eprintln!(
                "  monitor                           read transcript delta, update byte counts"
            );
            eprintln!("  check   <hook-name>               exit 0 if should inject, 1 if not");
            eprintln!(
                "  record  <hook-name>               record current monitor state as baseline"
            );
            eprintln!("  output  <hook-event> <context>    print CC hook JSON to stdout");
            eprintln!("  reset                             clear all state (post-compaction)");
            eprintln!("  parse   <path> <offset>           print \"nt_bytes th_bytes\" for transcript delta");
            if !other.is_empty() {
                bail!("unknown subcommand: {other}");
            }
            process::exit(2);
        }
    }
}

// ── Subcommand implementations ────────────────────────────────────────────────

/// `reinject monitor` — port of context-monitor.sh.
fn cmd_monitor() -> Result<()> {
    let input = read_stdin_json()?;

    // Skip sub-agents.
    if input.agent_id.is_some() {
        return Ok(());
    }

    let transcript_path = match input.transcript_path {
        Some(ref p) if !p.as_os_str().is_empty() => p.as_path(),
        _ => return Ok(()), // no transcript, nothing to do
    };

    let dir = resolve_state_dir(&input)?;
    update_monitor(&dir, transcript_path)
}

/// `reinject check <hook-name>` — exit 0 if should inject, exit 1 if skip.
fn cmd_check(hook_name: &str) -> Result<()> {
    let input = read_stdin_json()?;

    // Never inject in sub-agents.
    if input.agent_id.is_some() {
        process::exit(1);
    }

    let dir = resolve_state_dir(&input)?;
    let config = ThrottleConfig::default();

    match should_reinject(hook_name, &config, &dir)? {
        ThrottleDecision::Inject(reason) => {
            eprintln!("[INFO] reinject check: injecting {hook_name} ({reason:?})");
            // exit 0 = inject (default on success)
            Ok(())
        }
        ThrottleDecision::Skip => {
            process::exit(1);
        }
    }
}

/// `reinject record <hook-name>` — record current monitor state as the hook's baseline.
fn cmd_record(hook_name: &str) -> Result<()> {
    let input = read_stdin_json()?;
    let dir = resolve_state_dir(&input)?;

    // Read the current monitor status; default to zeros if monitor hasn't run.
    let status = reinject_core::read_monitor_status(&dir).unwrap_or_default();
    record(&dir, hook_name, &status)
}

/// `reinject output <hook-event> <context> [system-message]` — print CC hook JSON.
fn cmd_output(hook_event: &str, context: &str, system_message: Option<&str>) -> Result<()> {
    println!("{}", hook_output(hook_event, context, system_message));
    Ok(())
}

/// `reinject reset` — port of compact-reset.sh.
fn cmd_reset() -> Result<()> {
    let input = read_stdin_json()?;
    let dir = resolve_state_dir(&input)?;
    reset_state(&dir)?;
    eprintln!("[INFO] reinject: compaction detected, reset all state");
    Ok(())
}

/// `reinject parse <path> <offset>` — print "nt th" for the transcript delta.
fn cmd_parse(path: &Path, offset: u64) -> Result<()> {
    if !path.exists() {
        println!("0 0");
        return Ok(());
    }
    let (nt, th) = parse_transcript_delta(path, offset)?;
    println!("{nt} {th}");
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// JSON shape of the stdin hook input.
#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    agent_id: Option<String>,
    transcript_path: Option<PathBuf>,
}

/// Read and parse JSON from stdin.
fn read_stdin_json() -> Result<HookInput> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("failed to read stdin")?;
    let input: HookInput = if buf.trim().is_empty() {
        // No stdin — accept gracefully (e.g. direct invocation from tests).
        HookInput {
            session_id: None,
            agent_id: None,
            transcript_path: None,
        }
    } else {
        serde_json::from_str(&buf).context("failed to parse stdin JSON")?
    };
    Ok(input)
}

/// Resolve the state directory from session_id, falling back to the process ID.
fn resolve_state_dir(input: &HookInput) -> Result<PathBuf> {
    let key = match input.session_id.as_deref() {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => std::process::id().to_string(),
    };
    let dir = state_dir(&key);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create state dir {}", dir.display()))?;
    Ok(dir)
}

fn require_arg<'a>(args: &'a [String], idx: usize, usage: &str) -> Result<&'a str> {
    args.get(idx)
        .map(String::as_str)
        .with_context(|| format!("missing argument — usage: reinject {usage}"))
}

// ── Record-after-inject: expose MonitorStatus write for `cmd_record` ─────────
// The write_consumer_state re-export from reinject-core is used via `record()`.
// This test block validates CLI-level plumbing without running a full process.
#[cfg(test)]
mod tests {
    use super::*;
    use reinject_core::{write_monitor_status, MonitorStatus};
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn cmd_output_default_sys_msg() {
        // Just verify it doesn't panic and produces valid JSON.
        let out = hook_output("PreToolUse", "ctx", None);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(v.get("systemMessage").is_some());
    }

    #[test]
    fn record_writes_consumer_state() {
        let dir = tmp();
        let monitor = MonitorStatus {
            non_thinking_bytes: 42,
            thinking_bytes: 7,
            ..Default::default()
        };
        write_monitor_status(dir.path(), &monitor).unwrap();
        record(dir.path(), "my-hook", &monitor).unwrap();
        let saved = reinject_core::read_consumer_state(dir.path(), "my-hook").unwrap();
        assert_eq!(saved, monitor);
    }

    #[test]
    fn cmd_parse_nonexistent_file_prints_zero_zero() {
        // Smoke-test: parse on a missing file doesn't error.
        let dir = tmp();
        let missing = dir.path().join("nope.jsonl");
        cmd_parse(&missing, 0).unwrap();
    }

    #[test]
    fn resolve_state_dir_falls_back_to_pid_when_no_session() {
        let input = HookInput {
            session_id: None,
            agent_id: None,
            transcript_path: None,
        };
        let dir = resolve_state_dir(&input).unwrap();
        // The returned path must exist after the call.
        assert!(dir.exists());
        // Cleanup: remove it so we don't litter /tmp during test runs.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
