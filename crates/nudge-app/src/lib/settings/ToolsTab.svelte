<script>
  // Settings → Tools (§6.2, PLAN-step3 C.3): per-app Favorite/Normal/Hidden
  // three-way, Not-Tools list seeded by recommendation (high usage, never any
  // task's tool), and a read-only view of per-task ignore lists.
  import { onMount } from "svelte";
  import {
    listAppsForSelector,
    listNotToolCandidates,
    listTaskIgnores,
    setAppClass,
    refreshAppUsage,
  } from "../api.js";

  let apps = $state(/** @type {Array} */ ([]));
  let candidates = $state(/** @type {Array} */ ([]));
  let ignores = $state(/** @type {Array} */ ([]));
  let err = $state("");
  let refreshing = $state(false);

  async function load() {
    err = "";
    try {
      [apps, candidates, ignores] = await Promise.all([
        listAppsForSelector(),
        listNotToolCandidates(),
        listTaskIgnores(),
      ]);
    } catch (e) {
      err = String(e);
    }
  }

  onMount(load);

  async function refreshUsage() {
    refreshing = true;
    try {
      await refreshAppUsage(true);
      await load();
    } catch (e) {
      err = String(e);
    } finally {
      refreshing = false;
    }
  }

  async function classify(name, cls) {
    err = "";
    try {
      await setAppClass(name, cls);
      await load();
    } catch (e) {
      err = String(e);
    }
  }

  const CLASSES = ["favorite", "normal", "hidden"];
  const classed = $derived(apps.filter((a) => a.class !== "not_tool"));
  const notTools = $derived(apps.filter((a) => a.class === "not_tool"));
  const hours = (m) => (m >= 60 ? `${Math.round(m / 60)}h` : `${m}m`);
</script>

<section class="card">
  <div class="rowhead">
    <h2>Tools</h2>
    <button onclick={refreshUsage} disabled={refreshing}>
      {refreshing ? "Refreshing…" : "Refresh usage from ActivityWatch"}
    </button>
  </div>
  {#if err}<p class="error">{err}</p>{/if}
  {#if classed.length === 0}
    <p class="empty">No apps known yet — refresh usage, or add tools on a task.</p>
  {:else}
    <ul class="applist">
      {#each classed as a (a.name)}
        <li>
          <span class="name">{a.name}</span>
          {#if a.minutes_90d > 0}<span class="usage">{hours(a.minutes_90d)} / 90d</span>{/if}
          <span class="seg">
            {#each CLASSES as c}
              <button class:on={a.class === c} onclick={() => classify(a.name, c)}>{c}</button>
            {/each}
            <button class="nt" onclick={() => classify(a.name, "not_tool")}>not a tool</button>
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section class="card">
  <h2>Not-Tools</h2>
  <p class="hint">Apps that never count as working on anything.</p>
  {#if notTools.length}
    <ul class="applist">
      {#each notTools as a (a.name)}
        <li>
          <span class="name">{a.name}</span>
          <span class="seg"><button onclick={() => classify(a.name, "normal")}>restore</button></span>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">None yet.</p>
  {/if}
  {#if candidates.length}
    <h3>Suggested (high usage, never a task tool)</h3>
    <ul class="applist">
      {#each candidates as a (a.name)}
        <li>
          <span class="name">{a.name}</span>
          <span class="usage">{hours(a.minutes_90d)} / 90d</span>
          <span class="seg"><button onclick={() => classify(a.name, "not_tool")}>mark not-tool</button></span>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section class="card">
  <h2>Per-task ignores</h2>
  <p class="hint">Read-only — set at the check-in classification screen.</p>
  {#if ignores.length}
    <ul class="applist">
      {#each ignores as ig (ig.task_id + ig.app_name)}
        <li><span class="name">{ig.app_name}</span><span class="usage">on “{ig.task_title}”</span></li>
      {/each}
    </ul>
  {:else}
    <p class="empty">No ignores yet.</p>
  {/if}
</section>

<style>
  .rowhead { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  .rowhead h2 { margin: 0; }
  .applist { list-style: none; margin: 0; padding: 0; }
  .applist li {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.35rem 0;
    border-bottom: 1px solid var(--border, #333);
  }
  .applist li:last-child { border-bottom: none; }
  .name { flex: 1; }
  .usage { font-size: 12px; color: var(--fg-muted); white-space: nowrap; }
  .seg { display: flex; gap: 0.25rem; }
  .seg button { font-size: 12px; padding: 2px 8px; }
  .seg button.on { background: var(--accent); color: var(--accent-fg); border-color: transparent; }
  .seg button.nt { color: var(--danger); }
  h3 { font-size: 0.9rem; margin: 0.8rem 0 0.3rem; }
</style>
