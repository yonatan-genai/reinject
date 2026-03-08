#!/bin/bash
# Context rot prevention — consumer library for Claude Code hooks.
#
# The monitor hook (context-monitor.sh) runs on UserPromptSubmit and writes
# cumulative text byte counts to a status file. Consumer hooks source this
# library and call should_reinject() to compare the monitor's output against
# their own per-hook injection history. No JSONL parsing — just arithmetic.
#
# Usage (in your PreToolUse hook):
#   INPUT=$(cat)  # capture stdin FIRST
#   REINJECT_GROWTH_BYTES=52000  # your threshold (optional)
#   source "/path/to/should-reinject.sh"
#   if should_reinject "my-hook-name"; then
#     # output your additionalContext JSON
#   fi
#
# The caller MUST capture stdin before sourcing this library.
# After injection, call reinject_record "my-hook-name" to update state.
#
# Environment variables (all optional):
#   REINJECT_GROWTH_BYTES       — step 3 threshold in text bytes (default: 105000 ≈ 30K tokens)
#   REINJECT_RECENCY_THRESHOLD  — upper dead zone boundary % (default: 85)
#   REINJECT_PRIMACY_THRESHOLD  — lower dead zone boundary % (default: 15)
#   REINJECT_MIN_CONTEXT_BYTES  — min total text for position check (default: 21000 ≈ 6K tokens)
#
# Criticality tiers (set REINJECT_GROWTH_BYTES before sourcing):
#   High   (credentials, security):   52000  (~15K tokens at ÷3.5 ratio)
#   Medium (workflow, conventions):   105000  (~30K tokens)
#   Low    (nice-to-have reminders):  175000  (~50K tokens)

REINJECT_GROWTH_BYTES="${REINJECT_GROWTH_BYTES:-105000}"
REINJECT_RECENCY_THRESHOLD="${REINJECT_RECENCY_THRESHOLD:-85}"
REINJECT_PRIMACY_THRESHOLD="${REINJECT_PRIMACY_THRESHOLD:-15}"
REINJECT_MIN_CONTEXT_BYTES="${REINJECT_MIN_CONTEXT_BYTES:-21000}"

# Skip sub-agents — context reinjection is pointless in short-lived sub-agents.
# The monitor doesn't run there either, so there's no state to read.
_reinject_agent_id=$(printf '%s' "$INPUT" | jq -r '.agent_id // empty' 2>/dev/null)
_REINJECT_IS_SUBAGENT=""
[ -n "$_reinject_agent_id" ] && _REINJECT_IS_SUBAGENT=1

# Use session_id from hook input for state isolation (stable across the session).
# The caller must have done INPUT=$(cat) before sourcing this library.
# Falls back to $PPID for backwards compat (non-CC callers, older CC versions).
_reinject_session_id=$(printf '%s' "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
_REINJECT_STATE_DIR="${REINJECT_STATE_DIR:-/tmp/claude-reinject-${_reinject_session_id:-$PPID}}"
_REINJECT_MONITOR_FILE="$_REINJECT_STATE_DIR/monitor-status"

should_reinject() {
  local hook_name="$1"

  if [ -z "$hook_name" ]; then
    echo "[ERROR] should_reinject: hook_name required" >&2
    return 1
  fi

  # Never reinject in sub-agents
  [ -n "$_REINJECT_IS_SUBAGENT" ] && return 1

  local consumer_file="$_REINJECT_STATE_DIR/$hook_name"
  mkdir -p "$_REINJECT_STATE_DIR"

  # Read monitor status (written by context-monitor.sh on UserPromptSubmit)
  if [ ! -f "$_REINJECT_MONITOR_FILE" ]; then
    # Monitor hasn't run yet (first tool call before first prompt completes?)
    # Inject on first opportunity to be safe
    if [ ! -f "$consumer_file" ]; then
      reinject_record "$hook_name"
      return 0
    fi
    return 1
  fi

  local monitor_nt monitor_th
  monitor_nt=$(sed -n '1p' "$_REINJECT_MONITOR_FILE")
  monitor_th=$(sed -n '2p' "$_REINJECT_MONITOR_FILE")

  # ── Step 1: First run → inject ──
  if [ ! -f "$consumer_file" ]; then
    reinject_record "$hook_name"
    return 0
  fi

  # Read consumer's saved state (from its last injection)
  local saved_nt saved_th
  saved_nt=$(sed -n '1p' "$consumer_file")
  saved_th=$(sed -n '2p' "$consumer_file")

  # ── Step 3: Absolute growth (non-thinking text bytes since MY last injection) ──
  local delta_nt=$((monitor_nt - saved_nt))
  if [ "$delta_nt" -lt 0 ] 2>/dev/null; then
    # Monitor was reset (compaction) but consumer state wasn't cleaned up
    reinject_record "$hook_name"
    return 0
  fi

  if [ "$delta_nt" -gt "$REINJECT_GROWTH_BYTES" ] 2>/dev/null; then
    echo "[INFO] should-reinject: ${delta_nt} text bytes since last inject (threshold: ${REINJECT_GROWTH_BYTES}), re-injecting $hook_name" >&2
    return 0
  fi

  # ── Step 4: Dead zone position check ──
  local total_all=$((monitor_nt + monitor_th))
  if [ "$total_all" -gt "$REINJECT_MIN_CONTEXT_BYTES" ] 2>/dev/null; then
    local saved_all=$((saved_nt + saved_th))
    if [ "$total_all" -gt 0 ]; then
      local position_pct=$((saved_all * 100 / total_all))
      if [ "$position_pct" -gt "$REINJECT_PRIMACY_THRESHOLD" ] && \
         [ "$position_pct" -lt "$REINJECT_RECENCY_THRESHOLD" ]; then
        echo "[INFO] should-reinject: injection at ${position_pct}% in dead zone, re-injecting $hook_name" >&2
        return 0
      fi
    fi
  fi

  return 1
}

# Call this AFTER successful injection to record current monitor values
reinject_record() {
  local hook_name="$1"
  local consumer_file="$_REINJECT_STATE_DIR/$hook_name"
  mkdir -p "$_REINJECT_STATE_DIR"

  local monitor_nt=0 monitor_th=0
  if [ -f "$_REINJECT_MONITOR_FILE" ]; then
    monitor_nt=$(sed -n '1p' "$_REINJECT_MONITOR_FILE")
    monitor_th=$(sed -n '2p' "$_REINJECT_MONITOR_FILE")
  fi

  printf '%s\n%s\n' "${monitor_nt:-0}" "${monitor_th:-0}" > "$consumer_file"
}
