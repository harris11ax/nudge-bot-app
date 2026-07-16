<script>
  // Settings shell (PLAN-step3 C.4): sub-tabs. General keeps the existing
  // Google connect card; Tools and Style are the P3 additions (§6.2/§6.9).
  import GoogleConnect from "../GoogleConnect.svelte";
  import ToolsTab from "./ToolsTab.svelte";
  import StyleTab from "./StyleTab.svelte";

  const SUBTABS = [
    { id: "general", label: "General" },
    { id: "tools", label: "Tools" },
    { id: "style", label: "Style" },
  ];
  let sub = $state("general");
</script>

<header class="head"><h1>Settings</h1></header>

<nav class="subtabs">
  {#each SUBTABS as t}
    <button class:active={sub === t.id} onclick={() => (sub = t.id)}>{t.label}</button>
  {/each}
</nav>

{#if sub === "general"}
  <GoogleConnect />
  <div class="card">
    <p class="empty">Per-mode colors/sounds, snooze defaults, AW endpoint.</p>
  </div>
{:else if sub === "tools"}
  <ToolsTab />
{:else}
  <StyleTab />
{/if}

<style>
  .subtabs { display: flex; gap: 0.4rem; margin-bottom: 0.8rem; }
  .subtabs button { font-size: 13px; padding: 4px 12px; }
  .subtabs button.active { background: var(--accent); color: var(--accent-fg); border-color: transparent; }
</style>
