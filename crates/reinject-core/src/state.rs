//! State file read/write — monitor-status, consumer files, offsets.
//!
//! All state lives under `/tmp/claude-reinject-{session_id}/`.
//! Files are plain text, two newline-separated integers each.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};

/// Cumulative non-thinking and thinking byte counts written by the monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonitorStatus {
    /// Cumulative non-thinking text bytes observed in the transcript.
    pub non_thinking_bytes: u64,
    /// Cumulative thinking bytes observed in the transcript.
    pub thinking_bytes: u64,
}

/// Returns the path to `/tmp/claude-reinject-{session_id}/`.
pub fn state_dir(session_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/claude-reinject-{session_id}"))
}

/// Read the monitor-status file.  Returns `None` if the file does not exist.
pub fn read_monitor_status(state_dir: &Path) -> Option<MonitorStatus> {
    let path = state_dir.join("monitor-status");
    read_two_u64(&path).ok().map(|(nt, th)| MonitorStatus {
        non_thinking_bytes: nt,
        thinking_bytes: th,
    })
}

/// Write the monitor-status file, creating the state directory if needed.
pub fn write_monitor_status(state_dir: &Path, status: &MonitorStatus) -> Result<()> {
    let path = state_dir.join("monitor-status");
    write_two_u64(&path, status.non_thinking_bytes, status.thinking_bytes)
        .with_context(|| format!("failed to write monitor-status at {}", path.display()))
}

/// Read the per-hook consumer state file.  Returns `None` if the file does not exist.
pub fn read_consumer_state(state_dir: &Path, hook_name: &str) -> Option<MonitorStatus> {
    let path = state_dir.join(hook_name);
    read_two_u64(&path).ok().map(|(nt, th)| MonitorStatus {
        non_thinking_bytes: nt,
        thinking_bytes: th,
    })
}

/// Write the per-hook consumer state file.
pub fn write_consumer_state(
    state_dir: &Path,
    hook_name: &str,
    status: &MonitorStatus,
) -> Result<()> {
    let path = state_dir.join(hook_name);
    write_two_u64(&path, status.non_thinking_bytes, status.thinking_bytes)
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

/// Read two `u64` values from a file (first line, second line).
fn read_two_u64(path: &Path) -> Result<(u64, u64)> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut lines = content.lines();
    let a: u64 = lines
        .next()
        .unwrap_or("0")
        .trim()
        .parse()
        .with_context(|| format!("invalid u64 in first line of {}", path.display()))?;
    let b: u64 = lines
        .next()
        .unwrap_or("0")
        .trim()
        .parse()
        .with_context(|| format!("invalid u64 in second line of {}", path.display()))?;
    Ok((a, b))
}

/// Write two `u64` values to a file (first line, second line).
fn write_two_u64(path: &Path, a: u64, b: u64) -> Result<()> {
    let content = format!("{a}\n{b}\n");
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
