<script>
  import { onMount } from "svelte";
  import {
    quickAdd,
    createTask,
    store,
    refreshSuggestedTriggers,
    acceptSuggestion,
    dismissSuggestion,
    scanConnectors,
  } from "./store.svelte.js";

  onMount(refreshSuggestedTriggers);

  const sourceLabel = { manual: "Manual", gmail: "Gmail", gcal: "Calendar" };
  let suggestionErr = $state("");
  let scanning = $state(false);
  let scanMsg = $state("");

  async function scan() {
    suggestionErr = "";
    scanMsg = "";
    scanning = true;
    try {
      const s = await scanConnectors();
      scanMsg = `Added ${s.gcal_added} from Calendar, ${s.gmail_added} from Gmail (${s.gmail_scanned} scanned).`;
    } catch (err) {
      suggestionErr = String(err);
    } finally {
      scanning = false;
    }
  }

  async function accept(id) {
    suggestionErr = "";
    try {
      await acceptSuggestion(id);
    } catch (err) {
      suggestionErr = String(err);
    }
  }

  async function dismiss(id) {
    suggestionErr = "";
    try {
      await dismissSuggestion(id);
    } catch (err) {
      suggestionErr = String(err);
    }
  }

  // Quick-add: `text @ time [recur]`, parsed by nudge-core via the backend so the
  // grammar has exactly one implementation.
  let line = $state("");
  let quickErr = $state("");
  let quickOk = $state("");

  async function submitQuick(e) {
    e.preventDefault();
    quickErr = "";
    quickOk = "";
    try {
      await quickAdd(line);
      quickOk = `Added: ${line}`;
      line = "";
    } catch (err) {
      quickErr = String(err);
    }
  }

  // Fallback structured form for anything quick-add can't express (deadline,
  // description, type, mode override).
  let form = $state({
    title: "",
    desc: "",
    task_type: "",
    time: "", // HH:MM → minutes on submit
    recur: "once",
    deadline: "", // datetime-local → unix seconds on submit
    mode_override: "",
  });
  let formErr = $state("");
  let formOk = $state("");

  function toMinutes(hhmm) {
    if (!hhmm) return null;
    const [h, m] = hhmm.split(":").map(Number);
    return h * 60 + m;
  }

  async function submitForm(e) {
    e.preventDefault();
    formErr = "";
    formOk = "";
    if (!form.title.trim()) {
      formErr = "Title is required.";
      return;
    }
    const payload = {
      title: form.title.trim(),
      desc: form.desc,
      task_type: form.task_type,
      minutes: toMinutes(form.time),
      recur: form.recur.trim() || "once",
      deadline: form.deadline ? Math.floor(new Date(form.deadline).getTime() / 1000) : null,
      mode_override: form.mode_override || null,
    };
    try {
      await createTask(payload);
      formOk = `Added: ${payload.title}`;
      form = { title: "", desc: "", task_type: "", time: "", recur: "once", deadline: "", mode_override: "" };
    } catch (err) {
      formErr = String(err);
    }
  }
</script>

<header class="head"><h1>Triggers</h1></header>

<section class="card">
  <div class="suggested-head">
    <h2>Suggested</h2>
    <button onclick={scan} disabled={scanning}>{scanning ? "Scanning…" : "Scan Gmail + Calendar"}</button>
  </div>
  {#if suggestionErr}<p class="error">{suggestionErr}</p>{/if}
  {#if scanMsg}<p class="ok">{scanMsg}</p>{/if}
  {#if store.suggestedTriggers.length}
    <ul class="suggested-list">
      {#each store.suggestedTriggers as s (s.id)}
        <li class="suggested-row">
          <span class="badge">{sourceLabel[s.source] ?? s.source}</span>
          <span class="suggested-title">{s.title}</span>
          {#if s.deadline}
            <span class="suggested-deadline">{new Date(s.deadline * 1000).toLocaleString()}</span>
          {/if}
          <span class="suggested-actions">
            <button class="primary" onclick={() => accept(s.id)}>Accept</button>
            <button onclick={() => dismiss(s.id)}>Dismiss</button>
          </span>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint">No pending suggestions. Scan to pull actionable mail and upcoming events.</p>
  {/if}
</section>

<section class="card">
  <h2>Quick add</h2>
  <p class="hint">Format: <code>text @ time [recur]</code> — e.g. <code>gym @ 17:30 mon,wed,fri</code></p>
  <form onsubmit={submitQuick} class="quickadd">
    <input placeholder="gym @ 17:30 mon,wed,fri" bind:value={line} />
    <button class="primary" type="submit">Add</button>
  </form>
  {#if quickErr}<p class="error">{quickErr}</p>{/if}
  {#if quickOk}<p class="ok">{quickOk}</p>{/if}
</section>

<section class="card">
  <h2>Full trigger</h2>
  <form onsubmit={submitForm} class="grid">
    <label>Title<input bind:value={form.title} required /></label>
    <label>Type<input bind:value={form.task_type} placeholder="health, work…" /></label>
    <label>Time of day<input type="time" bind:value={form.time} /></label>
    <label>Recur<input bind:value={form.recur} placeholder="once / daily / mon,wed,fri" /></label>
    <label>Deadline<input type="datetime-local" bind:value={form.deadline} /></label>
    <label>Mode
      <select bind:value={form.mode_override}>
        <option value="">Auto (classify at edge)</option>
        <option value="off_task">Force strong (off-task)</option>
        <option value="on_task">Force soft (on-task)</option>
      </select>
    </label>
    <label class="wide">Description<textarea rows="2" bind:value={form.desc}></textarea></label>
    <div class="wide"><button class="primary" type="submit">Create trigger</button></div>
  </form>
  {#if formErr}<p class="error">{formErr}</p>{/if}
  {#if formOk}<p class="ok">{formOk}</p>{/if}
</section>

<style>
  .suggested-head { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  .suggested-head h2 { margin: 0; }
  .suggested-list { list-style: none; margin: 0; padding: 0; }
  .suggested-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0;
    border-bottom: 1px solid var(--border, #333);
  }
  .suggested-row:last-child { border-bottom: none; }
  .badge {
    font-size: 0.75rem;
    padding: 0.1rem 0.5rem;
    border-radius: 1rem;
    background: var(--accent-muted, #2a2a3a);
    color: var(--accent, #8ab4ff);
    white-space: nowrap;
  }
  .suggested-title { flex: 1; }
  .suggested-deadline { font-size: 0.85rem; opacity: 0.7; }
  .suggested-actions { display: flex; gap: 0.4rem; }
</style>
