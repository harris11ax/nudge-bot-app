<script>
  import { store, removeTask, clearOpenTask } from "./store.svelte.js";
  import { hhmm, recurLabel, deadlineLabel, countdown, isMissed } from "./format.js";
  import TaskPage from "./TaskPage.svelte";

  let { onOpenTriggers } = $props();

  let keyword = $state("");
  let typeFilter = $state("all");
  let range = $state("7"); // days for the variable list; "all" = no bound
  let selected = $state(/** @type {any} */ (null));

  // svc click-through (9d-ii): a task id routed in from App.svelte (cold-start
  // `--task` or a running-instance WM_COPYDATA ping) jumps straight to that
  // task's detail page.
  $effect(() => {
    if (store.openTaskId == null) return;
    const t = store.tasks.find((x) => x.id === store.openTaskId);
    if (t) selected = t;
    clearOpenTask();
  });

  const now = Date.now();
  const WEEK_MS = 7 * 24 * 3600 * 1000;

  // Distinct task types present, for the checkbox/select filter.
  const types = $derived([
    "all",
    ...new Set(store.tasks.map((t) => t.task_type).filter(Boolean)),
  ]);

  function matches(t) {
    const k = keyword.trim().toLowerCase();
    const hitK =
      !k ||
      t.title.toLowerCase().includes(k) ||
      (t.desc || "").toLowerCase().includes(k);
    const hitT = typeFilter === "all" || t.task_type === typeFilter;
    return hitK && hitT;
  }

  // Week list: deadline within 7 days OR already missed. Missed sorts first.
  const week = $derived(
    store.tasks
      .filter(matches)
      .filter((t) => t.deadline != null && t.deadline * 1000 - now <= WEEK_MS)
      .sort((a, b) => a.deadline - b.deadline)
  );

  // Variable list: everything not in the week list, bounded by the range filter.
  const variable = $derived(
    store.tasks
      .filter(matches)
      .filter((t) => !(t.deadline != null && t.deadline * 1000 - now <= WEEK_MS))
      .filter((t) => {
        // "all" → no bound. Otherwise keep deadline-less tasks plus any whose
        // deadline falls within the selected horizon.
        if (range === "all" || t.deadline == null) return true;
        const days = (t.deadline * 1000 - now) / (24 * 3600 * 1000);
        return days <= Number(range);
      })
  );
</script>

{#if selected}
  <TaskPage task={selected} onBack={() => (selected = null)} onDelete={onOpenTriggers} />
{:else}
  <header class="head">
    <h1>Planner</h1>
    <button class="primary" onclick={onOpenTriggers}>＋ New task</button>
  </header>

  <div class="filters">
    <input placeholder="Search title / description…" bind:value={keyword} />
    <select bind:value={typeFilter}>
      {#each types as t}<option value={t}>{t === "all" ? "All types" : t}</option>{/each}
    </select>
    <select bind:value={range} title="Variable-list horizon">
      <option value="14">2 weeks</option>
      <option value="30">1 month</option>
      <option value="all">All</option>
    </select>
  </div>

  {#if store.error}<p class="error">{store.error}</p>{/if}

  <section>
    <h2>This week</h2>
    {#if week.length === 0}
      <p class="empty">No deadlines in the next 7 days.</p>
    {:else}
      <ul class="tasks">
        {#each week as t (t.id)}
          <li class:missed={isMissed(t.deadline, now)}>
            <button class="rowmain" onclick={() => (selected = t)}>
              <span class="title">{t.title}</span>
              <span class="meta">
                {#if t.minutes != null}<span class="chip">{hhmm(t.minutes)}</span>{/if}
                <span class="chip">{recurLabel(t.recur)}</span>
                {#if t.deadline != null}
                  <span class="chip due">{deadlineLabel(t.deadline)} · {countdown(t.deadline, now)}</span>
                {/if}
              </span>
            </button>
            <button class="del" title="Delete" onclick={() => removeTask(t.id)}>✕</button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section>
    <h2>Later</h2>
    {#if variable.length === 0}
      <p class="empty">Nothing else in range.</p>
    {:else}
      <ul class="tasks">
        {#each variable as t (t.id)}
          <li>
            <button class="rowmain" onclick={() => (selected = t)}>
              <span class="title">{t.title}</span>
              <span class="meta">
                {#if t.minutes != null}<span class="chip">{hhmm(t.minutes)}</span>{/if}
                <span class="chip">{recurLabel(t.recur)}</span>
                {#if t.deadline != null}<span class="chip">{deadlineLabel(t.deadline)}</span>{/if}
              </span>
            </button>
            <button class="del" title="Delete" onclick={() => removeTask(t.id)}>✕</button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}
