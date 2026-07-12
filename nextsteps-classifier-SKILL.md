---
name: nextsteps-classifier
description: Classify and auto-insert new tasks into NEXTSTEPS.md with model tier, UNSKIPPABLE flags, and dependency ordering. Use whenever you want to add a task to the project's task list — describe what needs doing in plain English and this skill will format it, determine the right Claude model, detect blocking dependencies, and update the file. Trigger for "add a task", "new step", "next feature", "what's next", or when the user provides a task description and wants it added to the checklist.
compatibility: Filesystem MCP (read/write C:\Users\harri\Claude\Projects\nudge-bot\NEXTSTEPS.md)
---

# NEXTSTEPS Classifier

Automatically classify new tasks and maintain NEXTSTEPS.md with proper model tiers, dependency detection, and ordering.

## Workflow

### Step 1: Gather task info
Ask the user for:
1. **Task description** (what needs to be done)
2. **Time estimate** (rough: <30m, 1h, 2h, 4h, 8h, 1d+)
3. **Blocking notes** (does this task block any existing tasks? is it blocked by any?)

If the user has already provided this info in conversation, extract it and confirm before proceeding.

### Step 2: Analyze and classify

For each input task, perform a **Deep Reasoning Pass** to classify:

#### Model Tier Classification

Analyze the task against these categories. **At edge cases**, spend extra tokens to reason through which tier is truly minimal:

**Sonnet tier** (default, routine)
- Straightforward feature implementation (UI, API routes, data transformation)
- Predictable bugfixes with clear root cause
- Minor refactoring (rename, move, consolidate similar code)
- Test writing for existing features
- Documentation updates, comment cleanup
- Incremental additions to existing patterns
- Estimated effort: <4 hours

**Opus tier** (architectural, design-heavy)
- High-level system design or major refactor (touching 3+ core modules)
- Defining new patterns or abstractions
- Trade-off analysis across multiple approaches
- Cross-cutting concerns (logging, error handling, performance budgets)
- Estimated effort: >4 hours OR involves unknown unknowns

**Haiku tier** (trivial, obvious)
- Typo fixes, formatting, whitespace
- Pinpoint one-liner bugfixes (line number known, fix obvious)
- Trivial copy edits
- Single-file obvious refactors
- Estimated effort: <15 minutes

**Edge case reasoning:** If a task sits between tiers (e.g., "medium-sized feature with some design questions"), start with the lower tier but note the reasoning. Example: "Sonnet (with potential Opus follow-up if design questions emerge)" — give the user visibility into the uncertainty.

#### UNSKIPPABLE Detection

Read the current NEXTSTEPS.md. For the new task, ask:
- Does this task have incomplete **dependencies** — i.e., other tasks in the list depend on it?
- Would skipping this task **break or invalidate** any existing tasks?
- Is this task a **critical blocker** for multiple downstream items?

If yes to any, flag as `UNSKIPPABLE`.

Example: If task A is "thread task_id through state machine" and task B (already in list) is "wire task_id to app receiver", then A is unskippable (B depends on it).

#### Time Estimate

Bucket into: `<30m`, `~1h`, `~1.5h`, `~2h`, `~4h`, `~1d`, `~2d+`.

### Step 3: Reorder NEXTSTEPS.md

After classifying the new task:

1. **Identify all unskippable tasks** (both existing and new).
2. **Arrange them in dependency order** — if task X blocks task Y, X comes first.
3. **Place all other tasks after unskippable ones**, preserving their relative order.
4. **Validate**: Confirm no task depends on something that comes after it.

### Step 4: Renumber and sync task counter

After inserting the new task and reordering:

1. **Scan NEXTSTEPS.md for all incomplete tasks** (unchecked `[ ]` checkboxes).
2. **Find the first incomplete task** in the "## Next steps" section.
3. **Assign it step number 1** in its title (e.g., `**1 — Task title**`).
4. **Renumber all subsequent incomplete tasks** sequentially (2, 3, 4, …).
5. **If the first incomplete task is a substep** (e.g., "9d-ii" or "11a"):
   - Keep the parent task number in context (e.g., `**1 — 9d-ii (app half) — task-id threading**`)
   - Mark the parent task's main checkpoint as "in progress" if it has multiple substeps
   - Preserve completed substeps in the parent's notes for continuity

### Step 5: Insert into NEXTSTEPS.md

Format the new entry:
```markdown
- [ ] **N — Short title** | **Model** | [UNSKIPPABLE] | ~time
  Full description (2–3 lines).
  If there are specifics (key files, design note, call-outs), add them.
```

Append (or insert in the right position) to the "## Next steps" section. If the new task is unskippable and reordering is needed, move existing tasks down. Then immediately apply Step 4 renumbering.

### Step 6: Summarize for user

Show the user:
1. **New entry** (formatted, ready to commit)
2. **Model tier rationale** (why Sonnet/Opus/Haiku; note any edge cases)
3. **UNSKIPPABLE reasoning** (if flagged)
4. **Reordering summary** (if tasks were moved)
5. **Renumbering summary** (which task is now Step 1, any substep context preserved)
6. **Updated NEXTSTEPS.md preview** (show the Next steps section with the new entry in context and renumbered)

Ask the user to confirm before writing to disk. If they request changes (different tier, different time estimate, different title), adjust and re-show.

## Example 1: Adding a new task (no substeps)

**User input:**
```
Add this task: Build a Google Calendar read connector for the nudge-app. 
Need to fetch events, parse recurrence, show on the Calendar tab. 
Estimate: 6–8 hours. Might involve some design work on how recurring events map to the UI.
```

**Skill analysis:**
- **Model tier**: Opus (>4h, architectural: calendar API integration + recurrence logic + UI mapping)
- **Time**: ~1d
- **Blocking?**: Check NEXTSTEPS.md — does anything depend on Calendar? (e.g., if task 11–12 mentions "read-only calendar", this is unskippable for that.)
- **Unskippable**: Yes (if 11–12 list Calendar as P2 prerequisite)
- **Position**: Insert before task 11–12; renumber all subsequent tasks

**Output (to show user):**
```markdown
- [ ] **10 — GCal read + recurrence mapper** | **Opus** | UNSKIPPABLE | ~1d
  Fetch Google Calendar events (OAuth flow TBD).  Parse `RRULE` recurring rules; 
  map to `[deadline_start, deadline_end)` windows. Calendar.svelte read-only render + 
  fallback if GCal down. Design: minimal scope for P2 (write + connectors defer).
```

**Renumbering applied:**
```
- [ ] **1 — GCal read + recurrence mapper** | **Opus** | UNSKIPPABLE | ~1d
- [ ] **2 — UI-P2–P4 Calendar + connectors** | **Opus** | Planning run required.
- [ ] **3 — UI addendum (task tools, check-in flow...)** | **Opus** | Planning run required.
```

**Reasoning shown to user:**
> Model: **Opus** — 6–8h + architectural unknowns (OAuth, RRULE parsing logic, UI mapping strategy). Blocked on task 11–12 (Calendar tab expectation). Positioned before 11–12, making it Step 1.

## Example 2: Adding a substep to an in-progress task

**User input:**
```
Add this task: 9d-ii.5 — Sidebar Start button wiring. After task 9d-ii lands, 
the app receiver is ready but the Start button in the task page sidebar still 
doesn't fire. Connect it to the app's task-launch IPC. Estimate: 30 minutes.
Blocked by 9d-ii.
```

**Skill analysis:**
- **Model tier**: Sonnet (trivial, once parent 9d-ii is done)
- **Time**: <30m
- **Blocking?**: No (subtask, no other tasks depend on it)
- **UNSKIPPABLE**: No
- **Parent task**: 9d-ii is already marked UNSKIPPABLE; this subtask inherits priority but is sequenced after

**Output (to show user):**
```markdown
- [ ] **1 — 9d-ii (app half) — task-id threading & app receiver** | **Sonnet** | UNSKIPPABLE | ~1.5h
  [Original description, first substeps marked DONE or IN-PROGRESS, new substep listed]
  - 9d-ii.1: [existing context preserved]
  - 9d-ii.2: [existing context preserved]
  - 9d-ii.5: Sidebar Start button wiring. Connect to app's task-launch IPC. ~<30m
```

**Renumbering:**
> Parent task 9d-ii remains Step 1 (UNSKIPPABLE). New substep 9d-ii.5 is noted as a child. If all prior substeps are complete, mark parent as "in progress" rather than "to do" — continuity signal that work is active.

**Reasoning shown to user:**
> Sonnet-tier subtask (trivial wiring). Depends on 9d-ii; positioned as Step 1.1. Parent task 9d-ii stays Step 1 and marked IN-PROGRESS to signal active work.

## Notes

- **Preserving context**: Always read the full NEXTSTEPS.md before classifying. Understand the project's phase, recent work, and what's already in flight.
- **Explainability**: The user sees the reasoning, not just the output. This helps them override if needed.
- **Confirmation loop**: Never auto-write without showing the user the entry and letting them approve.
