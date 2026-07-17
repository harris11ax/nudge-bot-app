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

export const googleDisconnect = () => invoke("google_disconnect");

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

/** @returns {Promise<string|null>} the app's chosen write-target calendar id, or null if unpicked. */
export const primaryCalendar = () => invoke("primary_calendar");

/** Set the write-target calendar (Settings — Calendar picker). */
export const setPrimaryCalendar = (gcalId) => invoke("set_primary_calendar", { gcalId });

/** Create an event on the primary calendar (optimistic local write, then push). */
export const createEvent = (form) => invoke("create_event", { form });

/** Update an existing event's summary/time on the primary calendar. */
export const updateEvent = (eventId, form) => invoke("update_event", { eventId, form });

/** @returns {Promise<Array>} pending suggested triggers (connector inbox, 10d), newest first. */
export const listSuggestedTriggers = () => invoke("list_suggested_triggers");

/** Accept a suggestion: creates the live task, returns its new id. */
export const acceptSuggestedTrigger = (id) => invoke("accept_suggested_trigger", { id });

/** Dismiss a suggestion without creating a task. */
export const dismissSuggestedTrigger = (id) => invoke("dismiss_suggested_trigger", { id });

/**
 * Run the Gmail/GCal connectors (10e): refresh the calendar cache, then scan
 * upcoming events + recent actionable mail and deposit deduped candidates into
 * the Suggested inbox.
 * @returns {Promise<{gcal_added:number, gmail_added:number, gmail_scanned:number}>}
 */
export const runConnectors = () => invoke("run_connectors");

// --- Task tools + app classification (§6.2, PLAN-step3 P3) ---

/** @returns {Promise<Array<{name:string, minutes_90d:number, class:string}>>} every selectable app, usage-sorted desc. */
export const listAppsForSelector = () => invoke("list_apps_for_selector");

/** Replace a task's tool list wholesale. `tools` = [{app_name, kind}] with kind ∈ tool|ignore. */
export const setTaskTools = (taskId, tools) => invoke("set_task_tools_cmd", { taskId, tools });

/** @returns {Promise<Array<{app_name:string, kind:string}>>} a task's tool rows. */
export const listTaskTools = (taskId) => invoke("list_task_tools_cmd", { taskId });

/** Set an app's global class: favorite|normal|hidden|not_tool. */
export const setAppClass = (appName, cls) => invoke("set_app_class_cmd", { appName, class: cls });

/** @returns {Promise<Array>} explicitly-classified apps (Settings — Tools tab). */
export const listAppClasses = () => invoke("list_app_classes");

/** @returns {Promise<Array>} high-usage never-a-tool unclassified apps (Not-Tool seed). */
export const listNotToolCandidates = () => invoke("list_not_tool_candidates");

/** @returns {Promise<Array<{task_id:number, task_title:string, app_name:string}>>} all per-task ignore rows. */
export const listTaskIgnores = () => invoke("list_task_ignores");

/** @returns {Promise<Array<string>>} §6.9 completion-band colors ([] = defaults). */
export const getStyleBands = () => invoke("get_style_bands");

/** Persist the §6.9 completion-band colors. */
export const setStyleBands = (bands) => invoke("set_style_bands", { bands });

/** Refresh the app_usage cache from ActivityWatch (24h-capped unless force). */
export const refreshAppUsage = (force = false) => invoke("refresh_app_usage", { force });
