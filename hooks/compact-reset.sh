#!/bin/bash
# SessionStart compact hook — resets all state after compaction.
#
# When CC compacts context, old injections are summarized away and lose
# specific details. This clears monitor and consumer state so the next
# user prompt re-parses and the next relevant tool use re-injects.

STATE_DIR="${REINJECT_STATE_DIR:-/tmp/claude-reinject-$PPID}"

if [ -d "$STATE_DIR" ]; then
  rm -f "$STATE_DIR"/*
  echo "[INFO] context-hooks: compaction detected, reset all state" >&2
fi

exit 0
