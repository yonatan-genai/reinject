#!/bin/bash
# Context monitor — parses JSONL transcript delta on every user prompt.
#
# Runs on UserPromptSubmit (fires once per user message, before tool calls).
# Writes cumulative text byte counts to a status file that consumer hooks read.
#
# This is the ONLY hook that touches the JSONL transcript. Consumer hooks
# just read the status file — pure arithmetic, no parsing.
#
# Hook config (auto-registered via hooks.json):
#   "UserPromptSubmit": [{
#     "hooks": [{ "type": "command", "command": "/path/to/context-monitor.sh" }]
#   }]

set -euo pipefail

INPUT=$(cat)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
JQ_FILTER="$SCRIPT_DIR/../parsers/jq/extract-text-bytes.jq"
REINJECT_PARSER="${REINJECT_PARSER:-jq}"

STATE_DIR="${REINJECT_STATE_DIR:-/tmp/claude-reinject-$PPID}"
MONITOR_FILE="$STATE_DIR/monitor-status"
OFFSET_FILE="$STATE_DIR/monitor-offset"

mkdir -p "$STATE_DIR"

# Extract transcript path from hook input
transcript_path=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null)
if [ -z "$transcript_path" ] || [ ! -f "$transcript_path" ]; then
  exit 0  # no transcript, nothing to monitor
fi

# Read saved offset (0 if first run)
saved_offset=0
if [ -f "$OFFSET_FILE" ]; then
  saved_offset=$(cat "$OFFSET_FILE")
fi

# Current file size
current_size=$(wc -c < "$transcript_path" 2>/dev/null | tr -d ' ')

if [ "$current_size" -le "$saved_offset" ] 2>/dev/null; then
  # No growth since last check
  exit 0
fi

# Parse the JSONL delta
if [ "$REINJECT_PARSER" = "jq" ]; then
  delta_result=$(tail -c +$((saved_offset + 1)) "$transcript_path" 2>/dev/null | \
    tail -n +2 | \
    jq -R -r -f "$JQ_FILTER" 2>/dev/null | \
    awk '{nt+=$1; th+=$2} END {printf "%d %d", nt+0, th+0}')
else
  # Rust binary: expects <path> <offset>, outputs "nt_bytes th_bytes"
  delta_result=$("$REINJECT_PARSER" "$transcript_path" "$saved_offset")
fi

delta_nt=$(printf '%s' "$delta_result" | cut -d' ' -f1)
delta_th=$(printf '%s' "$delta_result" | cut -d' ' -f2)

# Read previous cumulative values
prev_nt=0
prev_th=0
if [ -f "$MONITOR_FILE" ]; then
  prev_nt=$(sed -n '1p' "$MONITOR_FILE")
  prev_th=$(sed -n '2p' "$MONITOR_FILE")
fi

# Update cumulative totals
new_nt=$((prev_nt + delta_nt))
new_th=$((prev_th + delta_th))

# Write updated status (consumers read this)
printf '%s\n%s\n' "$new_nt" "$new_th" > "$MONITOR_FILE"

# Save current offset for next delta
printf '%s' "$current_size" > "$OFFSET_FILE"

exit 0
