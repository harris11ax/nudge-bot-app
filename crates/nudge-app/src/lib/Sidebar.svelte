<script>
  import { store } from "./store.svelte.js";
  import { hhmm, recurLabel, deadlineLabel, countdown } from "./format.js";

  const now = Date.now();

  // To-do list sorted by deadline; deadline-less tasks sink to the bottom.
  const sorted = $derived(
    [...store.tasks].sort((a, b) => {
      const da = a.deadline ?? Infinity;
      const db = b.deadline ?? Infinity;
      return da - db;
    })
  );

  // Next deadline pinned at top (first task that actually has one).
  const next = $derived(sorted.find((t) => t.deadline != null));
</script>

<div class="side-head"><h2>To-do</h2></div>

{#if next}
  <div class="pinned">
    <div class="pin-label">Next deadline</div>
    <div class="pin-title">{next.title}</div>
    <div class="pin-when">{deadlineLabel(next.deadline)} · <b>{countdown(next.deadline, now)}</b></div>
    <button class="primary block" disabled title="Start (wired in P1 click-through)">Start</button>
  </div>
{:else}
  <p class="empty small">No deadlines set.</p>
{/if}

<ul class="mini">
  {#each sorted as t (t.id)}
    <li>
      <span class="mini-title">{t.title}</span>
      <span class="mini-meta">
        {#if t.minutes != null}{hhmm(t.minutes)} · {/if}{recurLabel(t.recur)}
        {#if t.deadline != null}· {countdown(t.deadline, now)}{/if}
      </span>
    </li>
  {/each}
</ul>
