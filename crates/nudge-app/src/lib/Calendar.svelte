<script>
  // Calendar tab (GOOGLE-PLAN.md 10b): month grid overlaying (a) cached GCal
  // events and (b) task deadlines, distinct styles. Refresh cadence is
  // server-enforced (§6.7 — launch/24h/manual); this component just calls
  // refresh_calendars on mount and lets the backend decide whether it's a
  // no-op. A failed refresh (offline, not connected) never blocks rendering —
  // cached events still show, just flagged with the offline banner.
  import { onMount } from "svelte";
  import { store, refresh as refreshTasks } from "./store.svelte.js";
  import {
    listCalendars,
    refreshCalendars,
    listEvents,
    googleLastRefresh,
    createEvent,
    updateEvent,
    deleteEvent,
    addTaskForEvent,
  } from "./api.js";

  const DAY_MS = 24 * 3600 * 1000;
  const WEEKDAY_LABELS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

  let cursor = $state(startOfMonth(new Date()));
  let calendars = $state(/** @type {Array} */ ([]));
  let events = $state(/** @type {Array} */ ([]));
  let lastRefresh = $state(/** @type {number | null} */ (null));
  let offline = $state(false);
  let refreshing = $state(false);
  let loadErr = $state("");

  function startOfMonth(d) {
    return new Date(d.getFullYear(), d.getMonth(), 1);
  }

  // Grid always shows full weeks (Monday start), padding into neighboring months.
  const gridDays = $derived(buildGrid(cursor));

  function buildGrid(monthStart) {
    const firstWeekday = (monthStart.getDay() + 6) % 7; // 0=Mon..6=Sun
    const gridStart = new Date(monthStart);
    gridStart.setDate(gridStart.getDate() - firstWeekday);
    const days = [];
    for (let i = 0; i < 42; i++) {
      const d = new Date(gridStart);
      d.setDate(gridStart.getDate() + i);
      days.push(d);
    }
    return days;
  }

  async function loadCalendars() {
    try {
      calendars = await listCalendars();
    } catch (err) {
      loadErr = String(err);
    }
  }

  async function loadEvents() {
    const from = Math.floor(gridDays[0].getTime() / 1000);
    const to = Math.floor((gridDays[gridDays.length - 1].getTime() + DAY_MS) / 1000);
    try {
      events = await listEvents(from, to);
    } catch (err) {
      loadErr = String(err);
    }
  }

  async function loadStamp() {
    try {
      lastRefresh = await googleLastRefresh();
    } catch {
      // Non-fatal — the stamp just stays blank.
    }
  }

  async function doRefresh(force) {
    refreshing = true;
    try {
      await refreshCalendars(force);
      offline = false;
      await loadCalendars();
      await loadStamp();
    } catch (err) {
      // Expected when not connected / offline — cached events below still render.
      offline = true;
      loadErr = String(err);
    } finally {
      refreshing = false;
      await loadEvents();
    }
  }

  function prevMonth() {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() - 1, 1);
  }
  function nextMonth() {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 1);
  }

  $effect(() => {
    // Re-read from cache whenever the visible month range changes.
    void cursor;
    loadEvents();
  });

  onMount(async () => {
    await loadCalendars();
    await loadStamp();
    await doRefresh(false);
  });

  function dayKey(d) {
    return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
  }

  // Events touching a given day (local-time day boundaries).
  function eventsOn(d) {
    const dayStart = d.getTime();
    const dayEnd = dayStart + DAY_MS;
    return events.filter(
      (e) => e.start_unix * 1000 < dayEnd && e.end_unix * 1000 > dayStart
    );
  }

  // Task deadlines landing on a given day.
  function deadlinesOn(d) {
    const dayStart = d.getTime();
    const dayEnd = dayStart + DAY_MS;
    return store.tasks.filter(
      (t) => t.deadline != null && t.deadline * 1000 >= dayStart && t.deadline * 1000 < dayEnd
    );
  }

  function calColor(calendarId) {
    return calendars.find((c) => c.gcal_id === calendarId)?.bg_color || "";
  }

  function eventTime(e) {
    if (e.all_day) return "";
    return new Date(e.start_unix * 1000).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  const monthLabel = $derived(
    cursor.toLocaleDateString(undefined, { month: "long", year: "numeric" })
  );
  const today = new Date();
  today.setHours(0, 0, 0, 0);

  const stampLabel = $derived(
    lastRefresh == null
      ? "never refreshed"
      : `refreshed ${new Date(lastRefresh * 1000).toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}`
  );

  // --- create/edit dialog (10c: write path, primary calendar only) ---

  let dialog = $state(/** @type {null | object} */ (null));
  let dialogErr = $state("");
  let dialogSaving = $state(false);

  function pad2(n) {
    return String(n).padStart(2, "0");
  }
  function isoDateLocal(d) {
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
  }
  // All-day events are stored with UTC-midnight civil-date bounds (matches the
  // backend's `unix_to_date_only`/`parse_date_only`) — decode with UTC getters.
  function isoDateUtc(unixSec) {
    const d = new Date(unixSec * 1000);
    return `${d.getUTCFullYear()}-${pad2(d.getUTCMonth() + 1)}-${pad2(d.getUTCDate())}`;
  }
  function hhmmLocal(d) {
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }

  function openCreate(d) {
    dialog = {
      mode: "create",
      eventId: null,
      summary: "",
      date: isoDateLocal(d),
      allDay: false,
      startTime: "09:00",
      endTime: "10:00",
    };
    dialogErr = "";
  }

  function openEdit(e) {
    dialog = {
      mode: "edit",
      eventId: e.event_id,
      summary: e.summary,
      date: e.all_day ? isoDateUtc(e.start_unix) : isoDateLocal(new Date(e.start_unix * 1000)),
      allDay: e.all_day,
      startTime: e.all_day ? "09:00" : hhmmLocal(new Date(e.start_unix * 1000)),
      endTime: e.all_day ? "10:00" : hhmmLocal(new Date(e.end_unix * 1000)),
    };
    dialogErr = "";
  }

  function closeDialog() {
    dialog = null;
  }

  function onDialogKeydown(ev) {
    if (ev.key === "Escape") closeDialog();
  }

  // Enter/Space activation for the non-<button> clickable grid cells below
  // (a day cell can't itself be a <button> since it contains event/deadline
  // items that are buttons too — nested buttons are invalid HTML).
  function activateOnKey(fn) {
    return (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        fn(ev);
      }
    };
  }

  async function submitDialog(ev) {
    ev.preventDefault();
    if (!dialog.summary.trim()) {
      dialogErr = "Title is required.";
      return;
    }
    const [y, m, d] = dialog.date.split("-").map(Number);
    let start_unix, end_unix;
    if (dialog.allDay) {
      start_unix = Math.floor(Date.UTC(y, m - 1, d) / 1000);
      end_unix = Math.floor(Date.UTC(y, m - 1, d + 1) / 1000);
    } else {
      start_unix = Math.floor(new Date(`${dialog.date}T${dialog.startTime}`).getTime() / 1000);
      end_unix = Math.floor(new Date(`${dialog.date}T${dialog.endTime}`).getTime() / 1000);
    }
    if (end_unix <= start_unix) {
      dialogErr = "End must be after start.";
      return;
    }
    const payload = { summary: dialog.summary.trim(), start_unix, end_unix, all_day: dialog.allDay };
    dialogSaving = true;
    dialogErr = "";
    try {
      if (dialog.mode === "create") {
        await createEvent(payload);
      } else {
        await updateEvent(dialog.eventId, payload);
      }
      dialog = null;
      await loadEvents();
    } catch (err) {
      dialogErr = String(err);
    } finally {
      dialogSaving = false;
    }
  }

  // §7.2 event-click menu — Delete Event. Pushes the delete to Google, drops the
  // cached row, and unbinds any task that referenced it; refreshes the grid and
  // the task store (a bound task's Calendar-event field flips to "—").
  // §7.2 event-click menu — Add Task. Opens a small form pre-filled from the
  // event (title + Deadline = event start); Create binds a new task to this event.
  let taskDialog = $state(/** @type {null | object} */ (null));
  let taskErr = $state("");
  let taskSaving = $state(false);

  function openAddTask(e) {
    const start = new Date(e.event_id ? e.start_unix * 1000 : Date.now());
    taskDialog = {
      eventId: e.event_id,
      title: e.summary,
      date: e.all_day ? isoDateUtc(e.start_unix) : isoDateLocal(start),
      time: e.all_day ? "09:00" : hhmmLocal(start),
    };
    taskErr = "";
    dialog = null; // close the edit dialog behind it
  }

  function closeTaskDialog() {
    taskDialog = null;
  }

  async function submitAddTask(ev) {
    ev.preventDefault();
    if (!taskDialog.title.trim()) {
      taskErr = "Title is required.";
      return;
    }
    // Deadline is a unix second at the chosen date + time-of-day (local).
    const deadline = Math.floor(
      new Date(`${taskDialog.date}T${taskDialog.time}`).getTime() / 1000
    );
    const form = {
      title: taskDialog.title.trim(),
      desc: "",
      deadline,
      task_type: "",
      minutes: null,
      recur: "once",
      mode_override: null,
      estimate_minutes: null,
    };
    taskSaving = true;
    taskErr = "";
    try {
      await addTaskForEvent(taskDialog.eventId, form);
      taskDialog = null;
      await refreshTasks();
    } catch (err) {
      taskErr = String(err);
    } finally {
      taskSaving = false;
    }
  }

  async function deleteDialogEvent() {
    if (!dialog || dialog.mode !== "edit") return;
    dialogSaving = true;
    dialogErr = "";
    try {
      await deleteEvent(dialog.eventId);
      dialog = null;
      await loadEvents();
      await refreshTasks();
    } catch (err) {
      dialogErr = String(err);
    } finally {
      dialogSaving = false;
    }
  }
</script>

<header class="head">
  <h1>Calendar</h1>
  <div class="cal-nav">
    <button onclick={prevMonth} title="Previous month">‹</button>
    <span class="cal-month">{monthLabel}</span>
    <button onclick={nextMonth} title="Next month">›</button>
  </div>
  <button class="primary" onclick={() => doRefresh(true)} disabled={refreshing}>
    {refreshing ? "Refreshing…" : "Refresh"}
  </button>
</header>

<p class="hint cal-stamp">
  {stampLabel}
  {#if offline}<span class="chip offline">offline — showing cached events</span>{/if}
</p>

<div class="cal-grid" class:offline>
  {#each WEEKDAY_LABELS as label}
    <div class="cal-weekday">{label}</div>
  {/each}
  {#each gridDays as d (dayKey(d))}
    {@const inMonth = d.getMonth() === cursor.getMonth()}
    {@const isToday = d.getTime() === today.getTime()}
    <div
      class="cal-day"
      class:out={!inMonth}
      class:today={isToday}
      role="button"
      tabindex="0"
      onclick={() => openCreate(d)}
      onkeydown={activateOnKey(() => openCreate(d))}
      title="Click to add an event"
    >
      <span class="cal-daynum">{d.getDate()}</span>
      <div class="cal-items">
        {#each deadlinesOn(d) as t (t.id)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="cal-item deadline" title={t.title} onclick={(ev) => ev.stopPropagation()}>⏱ {t.title}</div>
        {/each}
        {#each eventsOn(d) as e (e.event_id)}
          <div
            class="cal-item event"
            style={calColor(e.calendar_id) ? `--evt-color:${calColor(e.calendar_id)}` : ""}
            title={`${e.summary} (click to edit)`}
            role="button"
            tabindex="0"
            onclick={(ev) => {
              ev.stopPropagation();
              openEdit(e);
            }}
            onkeydown={activateOnKey((ev) => {
              ev.stopPropagation();
              openEdit(e);
            })}
          >
            {#if eventTime(e)}<span class="cal-time">{eventTime(e)}</span>{/if}
            {e.summary}
          </div>
        {/each}
      </div>
    </div>
  {/each}
</div>

{#if calendars.length > 0}
  <section class="card cal-legend">
    <h2>Calendars</h2>
    <ul class="cal-legend-list">
      {#each calendars as c (c.gcal_id)}
        <li class:muted={!c.selected}>
          <span class="cal-swatch" style={c.bg_color ? `background:${c.bg_color}` : ""}></span>
          {c.summary || c.gcal_id}
          {#if c.is_primary}<span class="chip">primary</span>{/if}
          {#if !c.selected}<span class="chip">hidden</span>{/if}
        </li>
      {/each}
    </ul>
    <p class="hint">Toggle which calendars overlay here from Settings.</p>
  </section>
{/if}

{#if dialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="overlay" onclick={closeDialog} onkeydown={onDialogKeydown}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog card" onclick={(ev) => ev.stopPropagation()}>
      <h2>{dialog.mode === "create" ? "New event" : "Edit event"}</h2>
      <form onsubmit={submitDialog} class="grid">
        <label class="wide">Title<input bind:value={dialog.summary} required /></label>
        <label class="wide inline-check">
          <input type="checkbox" bind:checked={dialog.allDay} />
          All day
        </label>
        <label class="wide">Date<input type="date" bind:value={dialog.date} required /></label>
        {#if !dialog.allDay}
          <label>Start<input type="time" bind:value={dialog.startTime} required /></label>
          <label>End<input type="time" bind:value={dialog.endTime} required /></label>
        {/if}
        {#if dialogErr}<p class="error wide">{dialogErr}</p>{/if}
        <div class="wide dialog-actions">
          {#if dialog.mode === "edit"}
            <button type="button" onclick={() => openAddTask(dialog && events.find((e) => e.event_id === dialog.eventId))} disabled={dialogSaving}>
              Add Task
            </button>
            <button type="button" class="danger" onclick={deleteDialogEvent} disabled={dialogSaving}>
              Delete
            </button>
          {/if}
          <button type="button" onclick={closeDialog}>Cancel</button>
          <button class="primary" type="submit" disabled={dialogSaving}>
            {dialogSaving ? "Saving…" : dialog.mode === "create" ? "Create" : "Save"}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if taskDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="overlay" onclick={closeTaskDialog} onkeydown={(ev) => ev.key === "Escape" && closeTaskDialog()}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog card" onclick={(ev) => ev.stopPropagation()}>
      <h2>Add task from event</h2>
      <form onsubmit={submitAddTask} class="grid">
        <label class="wide">Title<input bind:value={taskDialog.title} required /></label>
        <label>Deadline<input type="date" bind:value={taskDialog.date} required /></label>
        <label>Time<input type="time" bind:value={taskDialog.time} required /></label>
        {#if taskErr}<p class="error wide">{taskErr}</p>{/if}
        <div class="wide dialog-actions">
          <button type="button" onclick={closeTaskDialog}>Cancel</button>
          <button class="primary" type="submit" disabled={taskSaving}>
            {taskSaving ? "Saving…" : "Create task"}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}
