// Shared task state (Svelte 5 runes in a .svelte.js module). One source of truth
// the whole app reads/refreshes so a write in Triggers reflects instantly in the
// Planner and the right sidebar.
import { listTasks, addQuickadd, addTask, deleteTask } from "./api.js";

export const store = $state({
  tasks: /** @type {Array} */ ([]),
  loading: false,
  error: "",
  // Task id to route to (svc click-through, 9d-ii): cold-start `--task` or a
  // `nudge://open-task` event from an already-running instance. Consumed by
  // Planner's detail-page effect, then cleared.
  openTaskId: /** @type {number | null} */ (null),
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

export async function createTask(form) {
  await addTask(form);
  await refresh();
}

export async function removeTask(id) {
  await deleteTask(id);
  await refresh();
}
