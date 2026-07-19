<script>
  // Settings → Style (§6.9, PLAN-step3 C.4): color pickers for the three
  // logged/estimate completion bands (task_window StyleClass::Band(0..2)).
  // Persisted as a JSON array in meta.style_bands; empty = renderer defaults.
  import { onMount } from "svelte";
  import { getStyleBands, setStyleBands } from "../api.js";

  // Defaults mirror a low→high completion ramp; only used until first save.
  const DEFAULTS = ["#dc2626", "#d97706", "#15803d"];
  const LABELS = ["Band 0 — barely started (<50%)", "Band 1 — underway (50–90%)", "Band 2 — nearly done (≥90%)"];

  let bands = $state([...DEFAULTS]);
  let err = $state("");
  let ok = $state("");

  onMount(async () => {
    try {
      const saved = await getStyleBands();
      if (saved.length === 3) bands = saved;
    } catch (e) {
      err = String(e);
    }
  });

  async function save() {
    err = "";
    ok = "";
    try {
      await setStyleBands($state.snapshot(bands));
      ok = "Saved.";
    } catch (e) {
      err = String(e);
    }
  }

  async function reset() {
    bands = [...DEFAULTS];
    await save();
  }
</script>

<section class="card">
  <h2>Completion band colors</h2>
  <p class="hint">Row color by logged/estimate progress in the deadline-window task list.</p>
  {#if err}<p class="error">{err}</p>{/if}
  {#if ok}<p class="ok">{ok}</p>{/if}
  <ul class="bands">
    {#each bands as _, i}
      <li>
        <input type="color" bind:value={bands[i]} />
        <span>{LABELS[i]}</span>
        <span class="swatch" style="background:{bands[i]}"></span>
      </li>
    {/each}
  </ul>
  <div class="actions">
    <button class="primary" onclick={save}>Save</button>
    <button onclick={reset}>Reset to defaults</button>
  </div>
</section>

<style>
  .bands { list-style: none; margin: 0 0 0.8rem; padding: 0; }
  .bands li { display: flex; align-items: center; gap: 0.6rem; padding: 0.35rem 0; }
  .swatch { width: 60px; height: 14px; border-radius: 4px; }
  .actions { display: flex; gap: 0.5rem; }
</style>
