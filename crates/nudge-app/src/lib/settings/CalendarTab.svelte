<script>
  // §7.3 Settings → Calendar tab: all calendar/Google settings, moved out of
  // General. GoogleConnect holds OAuth connect/reconnect, the visible read
  // calendars (overlay checkboxes) and the primary (write) calendar picker.
  // This tab adds the refresh policy card (last-refreshed + manual Refresh,
  // §6.7) and the default auto-tied event duration (§7.2).
  import { onMount } from "svelte";
  import GoogleConnect from "../GoogleConnect.svelte";
  import {
    googleLastRefresh,
    refreshCalendars,
    getDefaultEventSecs,
    setDefaultEventSecs,
  } from "../api.js";

  let lastRefresh = $state(/** @type {number | null} */ (null));
  let refreshing = $state(false);
  let durationMin = $state(60);
  let saving = $state(false);
  let error = $state("");
  let saved = $state(false);

  function fmt(ts) {
    if (!ts) return "never";
    return new Date(ts * 1000).toLocaleString();
  }

  async function loadRefresh() {
    try {
      lastRefresh = await googleLastRefresh();
    } catch (err) {
      error = String(err);
    }
  }

  async function refreshNow() {
    refreshing = true;
    error = "";
    try {
      await refreshCalendars(true);
      await loadRefresh();
    } catch (err) {
      error = String(err);
    } finally {
      refreshing = false;
    }
  }

  async function loadDuration() {
    try {
      const secs = await getDefaultEventSecs();
      durationMin = Math.max(1, Math.round(secs / 60));
    } catch (err) {
      error = String(err);
    }
  }

  async function saveDuration() {
    const mins = Number(durationMin);
    if (!Number.isFinite(mins) || mins < 1) {
      error = "Duration must be at least 1 minute.";
      return;
    }
    saving = true;
    error = "";
    saved = false;
    try {
      await setDefaultEventSecs(Math.round(mins) * 60);
      saved = true;
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }

  onMount(() => {
    loadRefresh();
    loadDuration();
  });
</script>

<GoogleConnect />

<div class="card">
  <h2>Sync</h2>
  <p class="hint">
    Calendars refresh at most once every 24 h (§6.7); cached events stay available offline.
  </p>
  <p class="row">
    <span class="lbl">Last refreshed</span>
    <span>{fmt(lastRefresh)}</span>
  </p>
  <div class="btn-row">
    <button onclick={refreshNow} disabled={refreshing}>
      {refreshing ? "Refreshing…" : "Refresh now"}
    </button>
  </div>
</div>

<div class="card">
  <h2>New events</h2>
  <p class="hint">
    Default length of an event auto-created when a task with a Deadline is bound to your write
    calendar (§7.2). The event runs Deadline → Deadline + this duration.
  </p>
  <p class="row">
    <label class="lbl" for="evt-dur">Default duration (minutes)</label>
    <input id="evt-dur" type="number" min="1" step="1" bind:value={durationMin} />
    <button class="primary" onclick={saveDuration} disabled={saving}>
      {saving ? "Saving…" : "Save"}
    </button>
    {#if saved}<span class="ok">Saved</span>{/if}
  </p>
</div>

{#if error}<p class="error">{error}</p>{/if}

<style>
  .row { display: flex; align-items: center; gap: 0.6rem; }
  .lbl { min-width: 12rem; color: var(--muted, #888); }
  input[type="number"] { width: 6rem; }
</style>
