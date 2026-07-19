// Shared task state (Svelte 5 runes in a .svelte.js module). One source of truth
// the whole app reads/refreshes so a write in Triggers reflects instantly in the
// Planner and the right sidebar.
import {
  listTasks,
  addQuickadd,
  addTask,
  updateTask,
  deleteTask,
  listSuggestedTriggers,
  acceptSuggestedTrigger,
  dismissSuggestedTrigger,
  runConnectors,
  importTasks,
  confirmBulkUpload,
} from "./api.js";

export const store = $state({
  tasks: /** @type {Array} */ ([]),
  loading: false,
  error: "",
  // Task id to route to (svc click-through, 9d-ii): cold-start `--task` or a
  // `nudge://open-task` event from an already-running instance. Consumed by
  // Planner's detail-page effect, then cleared.
  openTaskId: /** @type {number | null} */ (null),
  // Pending connector-surfaced suggestions (Triggers tab — Suggested section, 10d).
  suggestedTriggers: /** @type {Array} */ ([]),
});

export function requestOpenTask(id) {
  store.openTaskId = id;
}

export function clearOpenTask() {
  store.openTaskId = null;
}

export async function refresh() {
  store.loading = true;
  store.error = "";
  try {
    store.tasks = await listTasks();
  } catch (e) {
    store.error = String(e);
  } finally {
    store.loading = false;
  }
}

export async function quickAdd(line) {
  await addQuickadd(line);
  await refresh();
}

/** Insert via the structured form; returns the stored TaskDto (with its id) so
 *  callers can attach child rows (e.g. §6.2 task_tools). */
export async function createTask(form) {
  const task = await addTask(form);
  await refresh();
  return task;
}

/** Update an existing task via the structured form (§7.2); returns the fresh
 *  TaskDto (with the possibly-refreshed event binding) and reloads the list. */
export async function saveTask(id, form) {
  const task = await updateTask(id, form);
  await refresh();
  return task;
}

export async function removeTask(id) {
  await deleteTask(id);
  await refresh();
}

export async function refreshSuggestedTriggers() {
  store.suggestedTriggers = await listSuggestedTriggers();
}

/** Accept a suggestion: creates the live task, then reconciles both lists. */
export async function acceptSuggestion(id) {
  await acceptSuggestedTrigger(id);
  await Promise.all([refresh(), refreshSuggestedTriggers()]);
}

/** Dismiss a suggestion; only the inbox needs reconciling. */
export async function dismissSuggestion(id) {
  await dismissSuggestedTrigger(id);
  await refreshSuggestedTriggers();
}

/**
 * Run the Gmail/GCal connectors (10e), then reload the inbox to show whatever
 * was deposited. Returns the run summary so the caller can toast counts.
 */
/** Import an approved batch of rows in one transaction, then reload the task list. */
export async function importTaskBatch(rows) {
  const count = await importTasks(rows);
  await refresh();
  return count;
}

/** Confirm a bulk upload (resolve hierarchy + insert in one tx), then reload. */
export async function bulkUploadConfirm(rows) {
  const count = await confirmBulkUpload(rows);
  await refresh();
  return count;
}

export async function scanConnectors() {
  const summary = await runConnectors();
  await refreshSuggestedTriggers();
  return summary;
}
