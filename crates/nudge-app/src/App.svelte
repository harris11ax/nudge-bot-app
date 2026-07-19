<script>
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { refresh, requestOpenTask } from "./lib/store.svelte.js";
  import { getPendingTask } from "./lib/api.js";
  import Planner from "./lib/Planner.svelte";
  import Triggers from "./lib/Triggers.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Placeholder from "./lib/Placeholder.svelte";
  import Calendar from "./lib/Calendar.svelte";
  import Settings from "./lib/settings/Settings.svelte";

  // Left-rail tabs (UI-PLAN §2). Planner + Triggers are live in P1; Calendar
  // (read-only) lands in P2; write-to-primary is P3. Settings/Log come later.
  const TABS = [
    { id: "planner", label: "Planner", icon: "▣" },
    { id: "calendar", label: "Calendar", icon: "▦" },
    { id: "triggers", label: "Tasks", icon: "＋" },
    { id: "settings", label: "Settings", icon: "⚙" },
    { id: "log", label: "Log", icon: "≡" },
  ];

  let active = $state("planner");
  let collapsed = $state(false);

  // Right sidebar width, persisted across launches.
  let sidebarW = $state(Number(localStorage.getItem("sidebarW")) || 300);
  let dragging = false;

  function startDrag(e) {
    dragging = true;
    e.preventDefault();
    const move = (ev) => {
      if (!dragging) return;
      // Distance from the window's right edge → sidebar width.
      const w = Math.min(560, Math.max(200, window.innerWidth - ev.clientX));
      sidebarW = w;
    };
    const up = () => {
      dragging = false;
      localStorage.setItem("sidebarW", String(sidebarW));
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  // svc click-through (9d-ii): route to a task's detail page from either a
  // cold-start `--task <id>` (pulled once here) or a `WM_COPYDATA` ping while
  // already running (Rust subclass emits this same event).
  function openTask(id) {
    requestOpenTask(id);
    active = "planner";
  }

  onMount(async () => {
    await refresh();
    const pending = await getPendingTask();
    if (pending != null) openTask(pending);
    const unlisten = await listen("nudge://open-task", (e) => openTask(e.payload));
    return unlisten;
  });
</script>

<div class="app">
  <!-- LEFT: collapsible tab rail -->
  <nav class="rail" class:collapsed>
    <button class="collapse" onclick={() => (collapsed = !collapsed)} title="Toggle menu">
      {collapsed ? "▸" : "◂"}
    </button>
    {#each TABS as tab}
      <button
        class="tab"
        class:active={active === tab.id}
        onclick={() => (active = tab.id)}
        title={tab.label}
      >
        <span class="tab-icon">{tab.icon}</span>
        {#if !collapsed}<span class="tab-label">{tab.label}</span>{/if}
      </button>
    {/each}
  </nav>

  <!-- CENTER: active tab -->
  <main class="content">
    {#if active === "planner"}
      <Planner onOpenTriggers={() => (active = "triggers")} />
    {:else if active === "triggers"}
      <Triggers />
    {:else if active === "calendar"}
      <Calendar />
    {:else if active === "settings"}
      <Settings />
    {:else}
      <Placeholder title="Log" note="sessions.db history view (edges + outcomes)." />
    {/if}
  </main>

  <!-- resize handle + RIGHT sidebar -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="handle" onmousedown={startDrag} role="separator" aria-orientation="vertical"></div>
  <aside class="sidebar" style="width:{sidebarW}px">
    <Sidebar />
  </aside>
</div>
