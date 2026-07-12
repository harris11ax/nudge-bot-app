<script>
  import { removeTask } from "./store.svelte.js";
  import { hhmm, recurLabel, deadlineLabel, countdown } from "./format.js";

  // Task detail page (UI-PLAN §2 Planner). History from sessions.db is a later
  // phase; for P1 this shows the stored fields + delete.
  let { task, onBack, onDelete } = $props();
  const now = Date.now();

  async function del() {
    await removeTask(task.id);
    onBack?.();
    onDelete?.();
  }
</script>

<header class="head">
  <button class="ghost" onclick={onBack}>← Back</button>
  <h1>{task.title}</h1>
</header>

<div class="card detail">
  {#if task.desc}<p class="desc">{task.desc}</p>{/if}
  <dl>
    <dt>Type</dt><dd>{task.task_type || "—"}</dd>
    <dt>Time of day</dt><dd>{task.minutes != null ? hhmm(task.minutes) : "—"}</dd>
    <dt>Recur</dt><dd>{recurLabel(task.recur)}</dd>
    <dt>Deadline</dt>
    <dd>{task.deadline != null ? `${deadlineLabel(task.deadline)} (${countdown(task.deadline, now)})` : "—"}</dd>
    <dt>Mode</dt>
    <dd>{task.mode_override === "off_task" ? "Force strong" : task.mode_override === "on_task" ? "Force soft" : "Auto"}</dd>
    <dt>Source</dt><dd>{task.trigger_source}</dd>
  </dl>
  <div class="detail-actions">
    <button class="danger" onclick={del}>Delete task</button>
  </div>
  <p class="hint">History (sessions.db edges + outcomes) lands with the Log tab.</p>
</div>
