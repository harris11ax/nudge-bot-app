<script>
  import { removeTask, saveTask } from "./store.svelte.js";
  import { hhmm, recurLabel, deadlineLabel, countdown } from "./format.js";
  import { listTaskTools, setTaskTools, launchTool, launchTaskTools } from "./api.js";

  // Task detail page (UI-PLAN §2 Planner). §7.2: an inline edit form wired to
  // `update_task` (rewrites the user-editable fields, best-effort propagates
  // title+Deadline onto the bound Calendar Event server-side). History from
  // sessions.db is a later phase.
  let { task, onBack, onDelete } = $props();
  const now = Date.now();

  let editing = $state(false);
  let form = $state(/** @type {any} */ (null));
  let saveErr = $state("");
  let saving = $state(false);

  // §7.4c launch-from-task: a task's attached tools, each launchable on demand
  // (websites open the bound URL / derived host, exes ShellExecute unless
  // running). The URL field is editable per web tool and persists via setTaskTools.
  let tools = $state(/** @type {Array<{app_name:string,kind:string,url:?string}>} */ ([]));
  let toolMsg = $state("");

  // A per-site tool name (§7.4b `<exe> → <host>`) is a website; anything else is
  // an exe. Websites get the editable URL field; exes just launch.
  const isWeb = (t) => t.app_name.includes(" → ");
  const launchTools = $derived(tools.filter((t) => t.kind === "tool"));

  async function loadTools() {
    try {
      tools = await listTaskTools(task.id);
    } catch (e) {
      toolMsg = String(e);
    }
  }
  $effect(() => {
    task.id; // re-load when the viewed task changes
    loadTools();
  });

  async function saveToolUrls() {
    // Round-trip the whole list so edited URLs persist (server stores blank → NULL).
    await setTaskTools(task.id, $state.snapshot(tools));
  }

  async function doLaunch(appName) {
    toolMsg = "";
    try {
      await launchTool(task.id, appName);
    } catch (e) {
      toolMsg = String(e);
    }
  }

  async function doLaunchAll() {
    toolMsg = "";
    try {
      const n = await launchTaskTools(task.id);
      toolMsg = `Launched ${n} tool${n === 1 ? "" : "s"}.`;
    } catch (e) {
      toolMsg = String(e);
    }
  }

  async function del() {
    await removeTask(task.id);
    onBack?.();
    onDelete?.();
  }

  function pad2(n) {
    return String(n).padStart(2, "0");
  }

  // unix seconds → the `datetime-local` value (local civil time), or "" if unset.
  function toDatetimeLocal(unixSec) {
    if (unixSec == null) return "";
    const d = new Date(unixSec * 1000);
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }

  // §7.1 Deadline-first: the fire `minutes` (since local midnight) derives from
  // the Deadline's time-of-day — mirrors the create form in Triggers.svelte.
  function minutesFromDeadline(dl) {
    if (!dl) return null;
    const d = new Date(dl);
    return d.getHours() * 60 + d.getMinutes();
  }

  function startEdit() {
    form = {
      title: task.title,
      desc: task.desc || "",
      task_type: task.task_type || "",
      recur: task.recur || "once",
      deadline: toDatetimeLocal(task.deadline),
      mode_override: task.mode_override || "",
      estimate: task.estimate_minutes != null ? String(task.estimate_minutes) : "",
    };
    saveErr = "";
    editing = true;
  }

  function cancelEdit() {
    editing = false;
    form = null;
    saveErr = "";
  }

  async function submitEdit(e) {
    e.preventDefault();
    saveErr = "";
    if (!form.title.trim()) {
      saveErr = "Title is required.";
      return;
    }
    const payload = {
      title: form.title.trim(),
      desc: form.desc,
      task_type: form.task_type,
      minutes: minutesFromDeadline(form.deadline),
      recur: form.recur.trim() || "once",
      deadline: form.deadline ? Math.floor(new Date(form.deadline).getTime() / 1000) : null,
      mode_override: form.mode_override || null,
      estimate_minutes: form.estimate ? Number(form.estimate) : null,
    };
    saving = true;
    try {
      // The returned DTO carries the (possibly refreshed) event binding; adopt it
      // so the detail view reflects the save without a round-trip through props.
      task = await saveTask(task.id, payload);
      editing = false;
      form = null;
    } catch (err) {
      saveErr = String(err);
    } finally {
      saving = false;
    }
  }
</script>

<header class="head">
  <button class="ghost" onclick={onBack}>← Back</button>
  <h1>{task.title}</h1>
  {#if !editing}<button class="ghost" onclick={startEdit}>✎ Edit</button>{/if}
</header>

<div class="card detail">
  {#if editing}
    <form onsubmit={submitEdit} class="grid">
      <label>Title<input bind:value={form.title} required /></label>
      <label>Type<input bind:value={form.task_type} placeholder="health, work…" /></label>
      <label>Recur<input bind:value={form.recur} placeholder="once / daily / mon,wed,fri" /></label>
      <label>Deadline<input type="datetime-local" bind:value={form.deadline} /></label>
      <label>Mode
        <select bind:value={form.mode_override}>
          <option value="">Auto (classify at edge)</option>
          <option value="off_task">Force strong (off-task)</option>
          <option value="on_task">Force soft (on-task)</option>
        </select>
      </label>
      <label>Estimate (min)<input type="number" min="0" bind:value={form.estimate} placeholder="90" /></label>
      <label class="wide">Description<textarea rows="2" bind:value={form.desc}></textarea></label>
      {#if saveErr}<p class="error wide">{saveErr}</p>{/if}
      <div class="wide detail-actions">
        <button type="button" onclick={cancelEdit}>Cancel</button>
        <button class="primary" type="submit" disabled={saving}>{saving ? "Saving…" : "Save"}</button>
      </div>
    </form>
  {:else}
    {#if task.desc}<p class="desc">{task.desc}</p>{/if}
    <dl>
      <dt>Type</dt><dd>{task.task_type || "—"}</dd>
      <dt>Time of day</dt><dd>{task.minutes != null ? hhmm(task.minutes) : "—"}</dd>
      <dt>Recur</dt><dd>{recurLabel(task.recur)}</dd>
      <dt>Deadline</dt>
      <dd>{task.deadline != null ? `${deadlineLabel(task.deadline)} (${countdown(task.deadline, now)})` : "—"}</dd>
      <dt>Mode</dt>
      <dd>{task.mode_override === "off_task" ? "Force strong" : task.mode_override === "on_task" ? "Force soft" : "Auto"}</dd>
      <dt>Source</dt><dd>{task.task_source}</dd>
      <dt>Calendar event</dt><dd>{task.gcal_event_id ? "bound" : "—"}</dd>
    </dl>
    <div class="detail-actions">
      <button class="danger" onclick={del}>Delete task</button>
    </div>
    <p class="hint">History (sessions.db edges + outcomes) lands with the Log tab.</p>
  {/if}
</div>

{#if !editing && launchTools.length}
  <div class="card tools">
    <div class="tools-head">
      <h2>Tools</h2>
      <button class="primary" onclick={doLaunchAll}>Launch all</button>
    </div>
    <ul class="tool-list">
      {#each launchTools as t (t.app_name)}
        <li>
          <span class="tool-name" class:web={isWeb(t)}>{t.app_name}</span>
          {#if isWeb(t)}
            <input
              class="tool-url"
              type="url"
              placeholder="https://… (optional exact URL)"
              bind:value={t.url}
              onblur={saveToolUrls}
            />
          {/if}
          <button class="ghost" onclick={() => doLaunch(t.app_name)}>Launch</button>
        </li>
      {/each}
    </ul>
    {#if toolMsg}<p class="hint">{toolMsg}</p>{/if}
  </div>
{/if}
