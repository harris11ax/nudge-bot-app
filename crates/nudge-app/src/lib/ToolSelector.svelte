<script>
  // §6.2 rich Tools selector (PLAN-step3 C.2): searchable dropdown fed by
  // app_usage ⟕ app_classes, usage-sorted, favorites pinned, hidden behind a
  // toggle, "Add tool manually" for anything AW hasn't seen. Selected tools
  // render as removable chips. Reused by the new-task form (Triggers) and,
  // later, the P2 picker's "New task…" form.
  import { onMount } from "svelte";
  import { listAppsForSelector, refreshAppUsage } from "./api.js";

  /** @type {{ selected: Array<{app_name:string, kind:string}> }} */
  let { selected = $bindable([]) } = $props();

  let apps = $state(/** @type {Array} */ ([]));
  let filter = $state("");
  let open = $state(false);
  let showHidden = $state(false);

  onMount(async () => {
    // Best-effort usage refresh first (24h-capped, no-op when AW is down),
    // then load the selector list.
    try { await refreshAppUsage(false); } catch { /* AW optional */ }
    try { apps = await listAppsForSelector(); } catch { apps = []; }
  });

  const isPicked = (name) => selected.some((t) => t.app_name === name);

  // Dropdown rows: not-tools always excluded, hidden gated by the toggle,
  // already-picked excluded, substring filter, favorites pinned above the
  // usage-desc order the backend already provides.
  const rows = $derived(
    apps
      .filter((a) => a.class !== "not_tool")
      .filter((a) => showHidden || a.class !== "hidden")
      .filter((a) => !isPicked(a.name))
      .filter((a) => a.name.toLowerCase().includes(filter.trim().toLowerCase()))
      .toSorted((x, y) => (y.class === "favorite") - (x.class === "favorite"))
  );

  function pick(name) {
    selected = [...selected, { app_name: name, kind: "tool" }];
    filter = "";
  }

  function remove(name) {
    selected = selected.filter((t) => t.app_name !== name);
  }

  function addManual() {
    const name = filter.trim();
    if (name && !isPicked(name)) pick(name);
  }
</script>

<div class="toolsel">
  {#if selected.length}
    <div class="chips">
      {#each selected as t (t.app_name)}
        <span class="chip picked">
          {t.app_name}
          <button type="button" class="x" title="Remove" onclick={() => remove(t.app_name)}>✕</button>
        </span>
      {/each}
    </div>
  {/if}

  <input
    placeholder="Search apps… (e.g. code.exe)"
    bind:value={filter}
    onfocus={() => (open = true)}
  />

  {#if open}
    <div class="dropdown">
      <label class="toggle">
        <input type="checkbox" bind:checked={showHidden} /> show hidden
      </label>
      {#each rows.slice(0, 30) as a (a.name)}
        <button type="button" class="row" onclick={() => pick(a.name)}>
          <span class="name">{a.class === "favorite" ? "★ " : ""}{a.name}</span>
          {#if a.minutes_90d > 0}
            <span class="usage">{Math.round(a.minutes_90d / 60)}h / 90d</span>
          {/if}
        </button>
      {:else}
        <p class="none">No matching apps.</p>
      {/each}
      {#if filter.trim() && !rows.some((a) => a.name === filter.trim())}
        <button type="button" class="row manual" onclick={addManual}>
          ＋ Add tool manually: “{filter.trim()}”
        </button>
      {/if}
      <button type="button" class="row close" onclick={() => (open = false)}>Done</button>
    </div>
  {/if}
</div>

<style>
  .toolsel { display: flex; flex-direction: column; gap: 0.4rem; position: relative; }
  .chips { display: flex; flex-wrap: wrap; gap: 0.3rem; }
  .chip.picked {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 12px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
    padding: 2px 4px 2px 8px;
    border-radius: 1rem;
  }
  .x { border: none; background: none; color: inherit; padding: 0 4px; cursor: pointer; font-size: 11px; }
  .dropdown {
    border: 1px solid var(--border, #333);
    border-radius: var(--radius, 10px);
    max-height: 260px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .toggle { font-size: 12px; color: var(--fg-muted); padding: 4px 8px; display: flex; gap: 0.3rem; align-items: center; }
  .row {
    display: flex;
    justify-content: space-between;
    gap: 0.6rem;
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
    cursor: pointer;
  }
  .row:hover { background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .usage { font-size: 11px; color: var(--fg-muted); white-space: nowrap; }
  .manual, .close { color: var(--accent); font-size: 13px; }
  .none { font-size: 12px; color: var(--fg-muted); padding: 6px 10px; margin: 0; }
</style>
