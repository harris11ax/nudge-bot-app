<script>
  // Calendar tab (GOOGLE-PLAN.md 10b): month grid overlaying (a) cached GCal
  // events and (b) task deadlines, distinct styles. Refresh cadence is
  // server-enforced (§6.7 — launch/24h/manual); this component just calls
  // refresh_calendars on mount and lets the backend decide whether it's a
  // no-op. A failed refresh (offline, not connected) never blocks rendering —
  // cached events still show, just flagged with the offline banner.
  import { onMount } from "svelte";
  import { store } from "./store.svelte.js";
  import { listCalendars, refreshCalendars, listEvents, googleLastRefresh } from "./api.js";

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
    <div class="cal-day" class:out={!inMonth} class:today={isToday}>
      <span class="cal-daynum">{d.getDate()}</span>
      <div class="cal-items">
        {#each deadlinesOn(d) as t (t.id)}
          <div class="cal-item deadline" title={t.title}>⏱ {t.title}</div>
        {/each}
        {#each eventsOn(d) as e (e.event_id)}
          <div
            class="cal-item event"
            style={calColor(e.calendar_id) ? `--evt-color:${calColor(e.calendar_id)}` : ""}
            title={e.summary}
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
