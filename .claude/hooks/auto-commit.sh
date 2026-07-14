#!/usr/bin/env bash
# Stop-hook: auto-commit the working tree when a turn finishes.
# Skips the default branch (main/master) to honor the NEXTSTEPS "main is
# production-ready; work goes on feature branches" rule, and no-ops when there
# is nothing to commit. Best-effort: never blocks the turn.
set -u

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
case "$branch" in
  main|master|HEAD) exit 0 ;;   # never auto-commit to the default branch
esac

git add -A 2>/dev/null
git diff --cached --quiet 2>/dev/null && exit 0   # nothing staged → done

ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
git commit -q -m "auto: session snapshot $ts" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>" >/dev/null 2>&1 || true
exit 0
