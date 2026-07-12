// Thin wrapper over the Tauri command surface (src-tauri/src/lib.rs). Keeping the
// invoke() calls in one place means components never import from @tauri-apps
// directly and the command contract is greppable.
import { invoke } from "@tauri-apps/api/core";

/** @returns {Promise<Array>} all tasks, ordered by id */
export const listTasks = () => invoke("list_tasks");

/** Parse+insert a quick-add line; rejects with the inline error message. */
export const addQuickadd = (line) => invoke("add_quickadd", { line });

/** Insert via the structured fallback form. */
export const addTask = (form) => invoke("add_task", { form });

/** Delete a task by id. */
export const deleteTask = (id) => invoke("delete_task", { id });

/** One-shot pull of a cold-start `--task <id>` (9d-ii click-through), or null. */
export const getPendingTask = () => invoke("get_pending_task");

/** @returns {Promise<"not_configured"|"disconnected"|"connected">} Google OAuth state. */
export const googleStatus = () => invoke("google_status");

/** Run the PKCE consent flow (opens the system browser); rejects on cancel/timeout. */
export const googleConnect = () => invoke("google_connect");

/** @returns {Promise<Array>} calendars known locally (cached selection/primary state, works offline). */
export const listCalendars = () => invoke("list_calendars");

/** Toggle a calendar's overlay checkbox. */
export const setCalendarSelected = (gcalId, selected) =>
  invoke("set_calendar_selected", { gcalId, selected });

/** Pull calendars+events from Google, respecting the §6.7 24h cap unless `force`. Rejects on network/auth failure — cached data is untouched. */
export const refreshCalendars = (force = false) => invoke("refresh_calendars", { force });

/** @returns {Promise<Array>} cached events overlapping [from, to) (unix seconds). Offline-safe. */
export const listEvents = (from, to) => invoke("list_events", { from, to });

/** @returns {Promise<number|null>} last successful Google refresh (unix seconds), or null if never. */
export const googleLastRefresh = () => invoke("google_last_refresh");
