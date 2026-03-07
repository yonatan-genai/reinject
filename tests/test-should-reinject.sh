#!/bin/bash
# Integration tests for context-monitor.sh + should-reinject.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
LIB="$REPO_DIR/hooks/lib/should-reinject.sh"
MONITOR="$REPO_DIR/hooks/context-monitor.sh"
JQ_FILTER="$REPO_DIR/parsers/jq/extract-text-bytes.jq"
FIXTURE="$REPO_DIR/tests/fixtures/sample.jsonl"

PASS=0
FAIL=0

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    echo "  PASS: $label"
    PASS=$((PASS + 1))
  else
    echo "  FAIL: $label (expected='$expected', actual='$actual')"
    FAIL=$((FAIL + 1))
  fi
}

make_hook_input() {
  printf '{"session_id":"test","transcript_path":"%s","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}' "$1"
}

make_prompt_input() {
  printf '{"session_id":"test","transcript_path":"%s","cwd":"/tmp","hook_event_name":"UserPromptSubmit","prompt":"hello"}' "$1"
}

run_monitor() {
  local transcript="$1" state_dir="$2"
  make_prompt_input "$transcript" | REINJECT_STATE_DIR="$state_dir" bash "$MONITOR" 2>/dev/null
}

# ── Test 1: jq parser extracts correct byte counts ──
echo "Test 1: jq parser byte extraction"
result=$(jq -R -r -f "$JQ_FILTER" < "$FIXTURE" | \
  awk '{nt+=$1; th+=$2} END {printf "%d %d", nt, th}')
nt=$(echo "$result" | cut -d' ' -f1)
th=$(echo "$result" | cut -d' ' -f2)
assert_eq "non-thinking bytes > 0" "1" "$([ "$nt" -gt 0 ] && echo 1 || echo 0)"
assert_eq "thinking bytes > 0" "1" "$([ "$th" -gt 0 ] && echo 1 || echo 0)"
assert_eq "non-thinking > thinking" "1" "$([ "$nt" -gt "$th" ] && echo 1 || echo 0)"
echo "  (non_thinking=$nt, thinking=$th)"

# ── Test 2: jq parser handles malformed lines ──
echo "Test 2: jq parser resilience"
result=$(printf 'not json\n{"message":{"content":"valid line here"}}\n{truncated' | \
  jq -R -r -f "$JQ_FILTER" 2>/dev/null | \
  awk '{nt+=$1; th+=$2} END {printf "%d %d", nt, th}')
nt=$(echo "$result" | cut -d' ' -f1)
assert_eq "skips bad lines, parses valid" "1" "$([ "$nt" -gt 0 ] && echo 1 || echo 0)"

# ── Test 3: monitor writes status file ──
echo "Test 3: monitor writes cumulative bytes"
STATE_DIR=$(mktemp -d)
run_monitor "$FIXTURE" "$STATE_DIR"
assert_eq "monitor-status exists" "1" "$([ -f "$STATE_DIR/monitor-status" ] && echo 1 || echo 0)"
assert_eq "monitor-offset exists" "1" "$([ -f "$STATE_DIR/monitor-offset" ] && echo 1 || echo 0)"
mon_nt=$(sed -n '1p' "$STATE_DIR/monitor-status")
mon_th=$(sed -n '2p' "$STATE_DIR/monitor-status")
assert_eq "monitor nt > 0" "1" "$([ "$mon_nt" -gt 0 ] && echo 1 || echo 0)"
assert_eq "monitor th > 0" "1" "$([ "$mon_th" -gt 0 ] && echo 1 || echo 0)"
echo "  (monitor: nt=$mon_nt, th=$mon_th)"
rm -rf "$STATE_DIR"

# ── Test 4: monitor is idempotent (no double-counting) ──
echo "Test 4: monitor idempotent on same file"
STATE_DIR=$(mktemp -d)
run_monitor "$FIXTURE" "$STATE_DIR"
first_nt=$(sed -n '1p' "$STATE_DIR/monitor-status")
run_monitor "$FIXTURE" "$STATE_DIR"
second_nt=$(sed -n '1p' "$STATE_DIR/monitor-status")
assert_eq "no double counting" "$first_nt" "$second_nt"
rm -rf "$STATE_DIR"

# ── Test 5: consumer first run injects ──
echo "Test 5: consumer first run injects"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
run_monitor "$FIXTURE" "$STATE_DIR"
source "$LIB"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"
if should_reinject "test-hook" 2>/dev/null; then
  assert_eq "first run injects" "INJECT" "INJECT"
else
  assert_eq "first run injects" "INJECT" "SKIP"
fi
rm -rf "$STATE_DIR"

# ── Test 6: consumer second run without growth skips ──
echo "Test 6: no growth = no injection"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
run_monitor "$FIXTURE" "$STATE_DIR"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"
REINJECT_GROWTH_BYTES=999999999
should_reinject "test-hook" 2>/dev/null && reinject_record "test-hook" || true
if should_reinject "test-hook" 2>/dev/null; then
  assert_eq "no growth skips" "SKIP" "INJECT"
else
  assert_eq "no growth skips" "SKIP" "SKIP"
fi
rm -rf "$STATE_DIR"

# ── Test 7: consumer triggers when growth exceeds threshold ──
echo "Test 7: growth past threshold triggers"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"

# Simulate: consumer injected when monitor showed 0 bytes
printf '0\n0\n' > "$STATE_DIR/test-hook"
# Monitor now shows significant growth
printf '60000\n10000\n' > "$STATE_DIR/monitor-status"

REINJECT_GROWTH_BYTES=52000  # High tier
if should_reinject "test-hook" 2>/dev/null; then
  assert_eq "growth triggers injection" "INJECT" "INJECT"
else
  assert_eq "growth triggers injection" "INJECT" "SKIP"
fi
rm -rf "$STATE_DIR"

# ── Test 8: compact reset clears everything ──
echo "Test 8: compact reset clears state"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
run_monitor "$FIXTURE" "$STATE_DIR"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"
should_reinject "test-hook" 2>/dev/null && reinject_record "test-hook" || true

before_files=$(ls "$STATE_DIR" | wc -l | tr -d ' ')
REINJECT_STATE_DIR="$STATE_DIR" bash "$REPO_DIR/hooks/compact-reset.sh" 2>/dev/null
after_files=$(ls "$STATE_DIR" 2>/dev/null | wc -l | tr -d ' ')
assert_eq "files exist before reset" "1" "$([ "$before_files" -gt 0 ] && echo 1 || echo 0)"
assert_eq "files gone after reset" "0" "$after_files"
rm -rf "$STATE_DIR"

# ── Test 9: dead zone position triggers ──
echo "Test 9: dead zone position triggers"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"

# Consumer injected at 10KB total text
printf '5000\n5000\n' > "$STATE_DIR/test-hook"
# Monitor now shows 50KB total — consumer is at 10/50 = 20% (in dead zone)
printf '25000\n25000\n' > "$STATE_DIR/monitor-status"

REINJECT_GROWTH_BYTES=999999999  # disable step 3
REINJECT_MIN_CONTEXT_BYTES=21000
if should_reinject "test-hook" 2>/dev/null; then
  assert_eq "dead zone triggers injection" "INJECT" "INJECT"
else
  assert_eq "dead zone triggers injection" "INJECT" "SKIP"
fi
rm -rf "$STATE_DIR"

# ── Test 10: small context skips position check ──
echo "Test 10: small context skips dead zone check"
STATE_DIR=$(mktemp -d)
export REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_STATE_DIR="$STATE_DIR"
_REINJECT_MONITOR_FILE="$STATE_DIR/monitor-status"

# Consumer at 50% position but total only 10KB (below 21KB minimum)
printf '2500\n2500\n' > "$STATE_DIR/test-hook"
printf '5000\n5000\n' > "$STATE_DIR/monitor-status"

REINJECT_GROWTH_BYTES=999999999
REINJECT_MIN_CONTEXT_BYTES=21000
if should_reinject "test-hook" 2>/dev/null; then
  assert_eq "small context skips" "SKIP" "INJECT"
else
  assert_eq "small context skips" "SKIP" "SKIP"
fi
rm -rf "$STATE_DIR"

# ── Summary ──
echo ""
echo "Results: $PASS passed, $FAIL failed"
unset REINJECT_STATE_DIR
REINJECT_GROWTH_BYTES=105000
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
