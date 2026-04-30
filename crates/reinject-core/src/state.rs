//! State file read/write — monitor-status, consumer files, offsets.
//!
//! All state lives under `/tmp/claude-reinject-{session_id}/`.
//! Status files are plain text with up to three newline-separated values:
//! `non_thinking_bytes`, `thinking_bytes`, and (optionally) `usage_tokens`.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};

/// Monitor reading written after each transcript update.
///
/// `non_thinking_bytes` and `thinking_bytes` are cumulative byte counts (legacy
/// signal — they only ever grow within a single jsonl file). `usage_tokens`,
/// when present, is the authoritative live-window token count read from the
/// most recent `message.usage` block in the transcript and resets across
/// `/clear` and auto-compact boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonitorStatus {
    /// Cumulative non-thinking text bytes observed in the transcript.
    pub non_thinking_bytes: u64,
    /// Cumulative thinking bytes observed in the transcript.
    pub thinking_bytes: u64,
    /// Latest live-window token total from `message.usage`. `None` when the
    /// transcript has not yet produced an assistant turn with a usage block
    /// (fresh session) or the state file was written by an older binary.
    pub usage_tokens: Option<u64>,
}

/// Returns the path to `/tmp/claude-reinject-{session_id}/`.
pub fn state_dir(session_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/claude-reinject-{session_id}"))
}

/// Read the monitor-status file.  Returns `None` if the file does not exist.
pub fn read_monitor_status(state_dir: &Path) -> Option<MonitorStatus> {
    let path = state_dir.join("monitor-status");
    read_status(&path).ok()
}

/// Write the monitor-status file, creating the state directory if needed.
pub fn write_monitor_status(state_dir: &Path, status: &MonitorStatus) -> Result<()> {
    let path = state_dir.join("monitor-status");
    write_status(&path, status)
        .with_context(|| format!("failed to write monitor-status at {}", path.display()))
}

/// Read the per-hook consumer state file.  Returns `None` if the file does not exist.
pub fn read_consumer_state(state_dir: &Path, hook_name: &str) -> Option<MonitorStatus> {
    let path = state_dir.join(hook_name);
    read_status(&path).ok()
}

/// Write the per-hook consumer state file.
pub fn write_consumer_state(
    state_dir: &Path,
    hook_name: &str,
    status: &MonitorStatus,
) -> Result<()> {
    let path = state_dir.join(hook_name);
    write_status(&path, status)
        .with_context(|| format!("failed to write consumer state for hook {hook_name}"))
}

/// Read the monitor byte offset.  Returns `0` if the file does not exist.
pub fn read_offset(state_dir: &Path) -> u64 {
    let path = state_dir.join("monitor-offset");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Write the monitor byte offset.
pub fn write_offset(state_dir: &Path, offset: u64) -> Result<()> {
    let path = state_dir.join("monitor-offset");
    atomic_write(&path, offset.to_string().as_bytes())
        .with_context(|| format!("failed to write offset at {}", path.display()))
}

/// Remove all files in the state directory (reset after compaction).
pub fn reset_state(state_dir: &Path) -> Result<()> {
    if !state_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(state_dir)
        .with_context(|| format!("failed to read state dir {}", state_dir.display()))?
    {
        let entry = entry.with_context(|| "failed to read dir entry")?;
        let path = entry.path();
        if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Read a [`MonitorStatus`] from `path`.
///
/// File format is line-oriented:
///   line 1: non_thinking_bytes
///   line 2: thinking_bytes
///   line 3 (optional): usage_tokens
///
/// The third line is read for backwards compatibility — files written by older
/// versions only have two lines and parse as `usage_tokens = None`.
fn read_status(path: &Path) -> Result<MonitorStatus> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut lines = content.lines();
    let nt: u64 = lines
        .next()
        .unwrap_or("0")
        .trim()
        .parse()
        .with_context(|| format!("invalid u64 in first line of {}", path.display()))?;
    let th: u64 = lines
        .next()
        .unwrap_or("0")
        .trim()
        .parse()
        .with_context(|| format!("invalid u64 in second line of {}", path.display()))?;
    let usage_tokens = match lines.next() {
        Some(s) if !s.trim().is_empty() => Some(
            s.trim()
                .parse::<u64>()
                .with_context(|| format!("invalid u64 in third line of {}", path.display()))?,
        ),
        _ => None,
    };
    Ok(MonitorStatus {
        non_thinking_bytes: nt,
        thinking_bytes: th,
        usage_tokens,
    })
}

/// Write a [`MonitorStatus`] to `path`. Always writes 3 lines; when
/// `usage_tokens` is `None` the third line is empty so old readers ignore it.
fn write_status(path: &Path, s: &MonitorStatus) -> Result<()> {
    let third = match s.usage_tokens {
        Some(v) => v.to_string(),
        None => String::new(),
    };
    let content = format!(
        "{}\n{}\n{}\n",
        s.non_thinking_bytes, s.thinking_bytes, third
    );
    atomic_write(path, content.as_bytes())
}

/// Write bytes to `path` atomically: write to a `.tmp` sibling, then rename.
///
/// POSIX `rename(2)` is atomic — readers see either the old file or the new
/// file, never a partial write.
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn monitor_status_roundtrip() {
        let dir = tmp();
        let status = MonitorStatus {
            non_thinking_bytes: 1234,
            thinking_bytes: 567,
            ..Default::default()
        };
        write_monitor_status(dir.path(), &status).unwrap();
        let read_back = read_monitor_status(dir.path()).unwrap();
        assert_eq!(read_back, status);
    }

    #[test]
    fn monitor_status_missing_returns_none() {
        let dir = tmp();
        assert!(read_monitor_status(dir.path()).is_none());
    }

    #[test]
    fn consumer_state_roundtrip() {
        let dir = tmp();
        let status = MonitorStatus {
            non_thinking_bytes: 99,
            thinking_bytes: 11,
            ..Default::default()
        };
        write_consumer_state(dir.path(), "my-hook", &status).unwrap();
        let read_back = read_consumer_state(dir.path(), "my-hook").unwrap();
        assert_eq!(read_back, status);
    }

    #[test]
    fn consumer_state_missing_returns_none() {
        let dir = tmp();
        assert!(read_consumer_state(dir.path(), "nonexistent-hook").is_none());
    }

    #[test]
    fn offset_roundtrip() {
        let dir = tmp();
        write_offset(dir.path(), 8192).unwrap();
        assert_eq!(read_offset(dir.path()), 8192);
    }

    #[test]
    fn offset_missing_returns_zero() {
        let dir = tmp();
        assert_eq!(read_offset(dir.path()), 0);
    }

    #[test]
    fn reset_state_removes_files() {
        let dir = tmp();
        let status = MonitorStatus {
            non_thinking_bytes: 10,
            thinking_bytes: 5,
            ..Default::default()
        };
        write_monitor_status(dir.path(), &status).unwrap();
        write_consumer_state(dir.path(), "hook-a", &status).unwrap();
        write_offset(dir.path(), 100).unwrap();
        reset_state(dir.path()).unwrap();
        assert!(read_monitor_status(dir.path()).is_none());
        assert!(read_consumer_state(dir.path(), "hook-a").is_none());
        assert_eq!(read_offset(dir.path()), 0);
    }

    #[test]
    fn reset_state_nonexistent_dir_is_ok() {
        let dir = tmp();
        let nonexistent = dir.path().join("no-such-dir");
        assert!(reset_state(&nonexistent).is_ok());
    }
}
