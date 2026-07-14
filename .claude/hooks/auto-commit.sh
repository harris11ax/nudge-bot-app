#!/usr/bin/env bash
# Stop-hook: auto-commit the working tree when a turn finishes, on ANY branch
# (covers all work on the project). No-ops when there is nothing to commit.
# Best-effort: never blocks the turn.
set -u

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

git add -A 2>/dev/null
git diff --cached --quiet 2>/dev/null && exit 0   # nothing staged → done

ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
git commit -q -m "auto: session snapshot $ts" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>" >/dev/null 2>&1 || true
exit 0
