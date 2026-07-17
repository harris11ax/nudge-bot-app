<script>
  import { onMount } from "svelte";
  import {
    googleStatus,
    googleConnect,
    googleDisconnect,
    listCalendars,
    setCalendarSelected,
    primaryCalendar,
    setPrimaryCalendar,
  } from "./api.js";

  // 10a: OAuth plumbing (google/mod.rs + oauth.rs). 10b adds the per-calendar
  // overlay checkboxes below, driving what the Calendar tab renders. 10c adds
  // the primary-calendar picker: the single calendar create/edit events write to.
  let status = $state("not_configured");
  let connecting = $state(false);
  let error = $state("");
  let calendars = $state(/** @type {Array} */ ([]));
  let primary = $state(/** @type {string | null} */ (null));

  async function load() {
    status = await googleStatus();
    if (status === "connected") {
      await loadCalendars();
      await loadPrimary();
    }
  }

  async function loadCalendars() {
    try {
      calendars = await listCalendars();
    } catch (err) {
      error = String(err);
    }
  }

  async function loadPrimary() {
    try {
      primary = await primaryCalendar();
    } catch (err) {
      error = String(err);
    }
  }

  async function pickPrimary(gcalId) {
    try {
      await setPrimaryCalendar(gcalId);
      primary = gcalId;
    } catch (err) {
      error = String(err);
    }
  }

  async function connect() {
    connecting = true;
    error = "";
    try {
      await googleConnect();
      await load();
    } catch (err) {
      error = String(err);
    } finally {
      connecting = false;
    }
  }

  async function disconnect() {
    error = "";
    try {
      await googleDisconnect();
      calendars = [];
      primary = null;
      await load();
    } catch (err) {
      error = String(err);
    }
  }

  async function toggle(c) {
    const next = !c.selected;
    try {
      await setCalendarSelected(c.gcal_id, next);
      c.selected = next;
    } catch (err) {
      error = String(err);
    }
  }

  onMount(load);
</script>

<div class="card">
  <h2>Google</h2>
  {#if status === "not_configured"}
    <p class="hint">
      No OAuth client configured. Create a Desktop-app OAuth client in Google Cloud Console, then
      save <code>{"{"}"client_id": "...", "client_secret": "..."{"}"}</code> to
      <code>%LOCALAPPDATA%\nudge-bot\google_client.json</code>.
    </p>
  {:else}
    <p class={status === "connected" ? "ok" : "hint"}>
      {status === "connected" ? "Connected" : "Not connected"}
    </p>
    <div class="btn-row">
      <button class="primary" onclick={connect} disabled={connecting}>
        {connecting ? "Waiting for sign-in…" : status === "connected" ? "Reconnect" : "Connect Google"}
      </button>
      {#if status === "connected"}
        <button class="ghost" onclick={disconnect} disabled={connecting}>Reset connection</button>
      {/if}
    </div>
    {#if status === "connected"}
      <p class="hint">
        Gmail scan returning 403? Click <strong>Reset connection</strong>, then
        <strong>Reconnect</strong> and approve the Gmail permission to refresh the granted scopes.
      </p>
    {/if}
    {#if status === "connected" && calendars.length > 0}
      <h3 class="cal-settings-h">Overlay on Calendar tab</h3>
      <ul class="cal-checklist">
        {#each calendars as c (c.gcal_id)}
          <li>
            <label>
              <input type="checkbox" checked={c.selected} onchange={() => toggle(c)} />
              <span class="cal-swatch" style={c.bg_color ? `background:${c.bg_color}` : ""}></span>
              {c.summary || c.gcal_id}
              {#if c.is_primary}<span class="chip">primary</span>{/if}
            </label>
          </li>
        {/each}
      </ul>
      <h3 class="cal-settings-h">Write target</h3>
      <p class="hint">Event create/edit writes to exactly one calendar.</p>
      <ul class="cal-checklist">
        {#each calendars as c (c.gcal_id)}
          <li>
            <label>
              <input
                type="radio"
                name="primary-calendar"
                checked={primary === c.gcal_id}
                onchange={() => pickPrimary(c.gcal_id)}
              />
              <span class="cal-swatch" style={c.bg_color ? `background:${c.bg_color}` : ""}></span>
              {c.summary || c.gcal_id}
            </label>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
  {#if error}<p class="error">{error}</p>{/if}
</div>
