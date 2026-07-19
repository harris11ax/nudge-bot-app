<script>
  // CSV bulk-task import filter screen (PLAN-csv-import.md §3, P3). Upload/paste a
  // CSV → server validates each row (validate_csv_import) → per-row filter table
  // with inline edits + include toggles → import_tasks writes the approved batch
  // in one transaction. File I/O stays in the webview (FileReader / Blob) so no
  // tauri dialog/fs plugin is needed; the LLM shaping happens outside nudge-bot.
  import { validateCsvImport } from "./api.js";
  import { importTaskBatch } from "./store.svelte.js";

  const HEADER = "title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode";
  const CELL_FIELDS = HEADER.split(",");

  const LLM_PROMPT = `Convert my task list below into CSV for import into a task app.
Return ONLY the CSV — no prose, no code fences — with exactly this header row first:

${HEADER}

Rules:
- title: required, non-empty.
- description: optional free text.
- deadline: optional, "YYYY-MM-DD" or "YYYY-MM-DDTHH:MM" (local time).
- time_of_day: optional "HH:MM" (24h); omit if the deadline already has a time.
- recur: "once" (default) or a day list like "mon,wed,fri".
- task_type: optional label (e.g. work, health, errand).
- estimate_minutes: optional integer.
- mode: "on_task" or "off_task", or leave blank.
Quote any field containing a comma. My task list:
`;

  let rows = $state(/** @type {Array} */ ([]));
  let err = $state("");
  let importing = $state(false);
  let importMsg = $state("");
  let copied = $state(false);

  const readyIncludedCount = $derived(rows.filter((r) => r.valid && r.include).length);

  /** Build the editable-cell map for a row from a DTO's raw (field,value) pairs. */
  function cellsFromDto(dto) {
    const cells = {};
    for (const f of CELL_FIELDS) cells[f] = "";
    for (const [k, v] of dto.raw) cells[k] = v;
    return cells;
  }

  /** Escape a single CSV cell (quote if it holds a comma, quote, or newline). */
  function csvCell(v) {
    const s = v ?? "";
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  }

  /** Serialize one row's cells back to a canonical single-row CSV blob. */
  function rowToCsv(cells) {
    const line = CELL_FIELDS.map((f) => csvCell(cells[f])).join(",");
    return `${HEADER}\n${line}`;
  }

  async function validateBlob(text) {
    err = "";
    importMsg = "";
    try {
      const dtos = await validateCsvImport(text);
      rows = dtos.map((d) => ({
        cells: cellsFromDto(d),
        form: d.form,
        valid: d.valid,
        errors: d.errors,
        include: d.valid, // auto-uncheck invalid rows
      }));
      if (!rows.length) err = "No data rows found in that CSV.";
    } catch (e) {
      err = String(e);
    }
  }

  /** Re-validate a single edited row via the same server parser. */
  async function revalidate(i) {
    try {
      const dtos = await validateCsvImport(rowToCsv(rows[i].cells));
      const d = dtos[0];
      if (!d) return;
      rows[i].form = d.form;
      rows[i].valid = d.valid;
      rows[i].errors = d.errors;
      if (!d.valid) rows[i].include = false;
    } catch (e) {
      err = String(e);
    }
  }

  function onFile(e) {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => validateBlob(String(reader.result ?? ""));
    reader.readAsText(file);
    e.target.value = ""; // allow re-picking the same file
  }

  let pasteText = $state("");
  function validatePaste() {
    if (pasteText.trim()) validateBlob(pasteText);
  }

  function downloadTemplate() {
    const blob = new Blob([`${HEADER}\n`], { type: "text/csv" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "task-import-template.csv";
    a.click();
    URL.revokeObjectURL(url);
  }

  async function copyPrompt() {
    try {
      await navigator.clipboard.writeText(LLM_PROMPT);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch (e) {
      err = "Clipboard blocked — select the prompt text manually.";
    }
  }

  async function doImport() {
    err = "";
    importMsg = "";
    importing = true;
    try {
      const payload = rows.filter((r) => r.valid && r.include).map((r) => r.form);
      const n = await importTaskBatch(payload);
      importMsg = `Imported ${n} task${n === 1 ? "" : "s"}.`;
      rows = [];
      pasteText = "";
    } catch (e) {
      err = String(e);
    } finally {
      importing = false;
    }
  }
</script>

<header class="head"><h1>Import tasks (CSV)</h1></header>

<section class="card">
  <h2>1 · Prepare</h2>
  <p class="hint">
    Paste your task list into Claude or Gemini, ask it to return CSV with the header below, then
    upload the result. Extra columns are ignored; a wrong header shows rows as invalid.
  </p>
  <p class="hint"><code>{HEADER}</code></p>
  <div class="btnrow">
    <button onclick={downloadTemplate}>Download blank template.csv</button>
    <button onclick={copyPrompt}>{copied ? "Copied ✓" : "Copy LLM prompt"}</button>
  </div>
</section>

<section class="card">
  <h2>2 · Load a CSV</h2>
  <div class="btnrow">
    <label class="filebtn">
      Choose file…
      <input type="file" accept=".csv,text/csv" onchange={onFile} />
    </label>
    <span class="hint">or paste the CSV text:</span>
  </div>
  <textarea rows="4" placeholder={HEADER} bind:value={pasteText}></textarea>
  <div class="btnrow">
    <button onclick={validatePaste} disabled={!pasteText.trim()}>Validate pasted CSV</button>
  </div>
  {#if err}<p class="error">{err}</p>{/if}
  {#if importMsg}<p class="ok">{importMsg}</p>{/if}
</section>

{#if rows.length}
  <section class="card">
    <div class="filter-head">
      <h2>3 · Review &amp; import</h2>
      <span class="hint">{readyIncludedCount} of {rows.length} row(s) ready</span>
    </div>
    <div class="tablewrap">
      <table class="filter">
        <thead>
          <tr>
            <th></th>
            <th>Status</th>
            <th>Title</th>
            <th>Deadline</th>
            <th>Time</th>
            <th>Recur</th>
            <th>Type</th>
            <th>Est</th>
            <th>Mode</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r, i}
            <tr class:invalid={!r.valid}>
              <td>
                <input
                  type="checkbox"
                  bind:checked={r.include}
                  disabled={!r.valid}
                  title={r.valid ? "Include in import" : "Fix errors to include"}
                />
              </td>
              <td>
                {#if r.valid}
                  <span class="pill ok-pill">ready</span>
                {:else}
                  <span class="pill bad-pill" title={r.errors.join("; ")}>needs fix</span>
                {/if}
              </td>
              <td><input bind:value={r.cells.title} onchange={() => revalidate(i)} /></td>
              <td><input class="w" bind:value={r.cells.deadline} placeholder="YYYY-MM-DD" onchange={() => revalidate(i)} /></td>
              <td><input class="xs" bind:value={r.cells.time_of_day} placeholder="HH:MM" onchange={() => revalidate(i)} /></td>
              <td><input class="s" bind:value={r.cells.recur} onchange={() => revalidate(i)} /></td>
              <td><input class="s" bind:value={r.cells.task_type} onchange={() => revalidate(i)} /></td>
              <td><input class="xs" bind:value={r.cells.estimate_minutes} onchange={() => revalidate(i)} /></td>
              <td><input class="s" bind:value={r.cells.mode} onchange={() => revalidate(i)} /></td>
            </tr>
            {#if !r.valid && r.errors.length}
              <tr class="errrow"><td></td><td colspan="8" class="errcell">{r.errors.join("; ")}</td></tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
    <div class="btnrow">
      <button class="primary" onclick={doImport} disabled={importing || readyIncludedCount === 0}>
        {importing ? "Importing…" : `Import ${readyIncludedCount} ready row(s)`}
      </button>
    </div>
  </section>
{/if}

<style>
  .btnrow { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 8px; }
  textarea { width: 100%; box-sizing: border-box; font-family: monospace; font-size: 12px; }
  .filebtn {
    display: inline-block; padding: 6px 12px; border: 1px solid var(--border, #333);
    border-radius: 6px; cursor: pointer; font-size: 13px;
  }
  .filebtn:hover { filter: brightness(1.1); }
  .filebtn input { display: none; }
  .filter-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
  .filter-head h2 { margin: 0; }
  .tablewrap { overflow-x: auto; margin-top: 8px; }
  table.filter { border-collapse: collapse; width: 100%; font-size: 13px; }
  table.filter th { text-align: left; padding: 4px 6px; color: var(--fg-muted); font-weight: 600; }
  table.filter td { padding: 3px 6px; border-top: 1px solid var(--border, #2a2a2a); }
  tr.invalid td { background: var(--danger-muted, rgba(255,80,80,0.06)); }
  tr.errrow td { border-top: none; }
  .errcell { color: var(--danger); font-size: 12px; padding-top: 0; padding-bottom: 6px; }
  table.filter input { width: 9rem; box-sizing: border-box; font-size: 12px; padding: 2px 4px; }
  table.filter input.s { width: 6rem; }
  table.filter input.xs { width: 4rem; }
  table.filter input.w { width: 11rem; }
  .pill { font-size: 11px; padding: 1px 8px; border-radius: 1rem; white-space: nowrap; }
  .ok-pill { background: var(--ok-muted, rgba(80,200,120,0.15)); color: var(--ok, #4caf70); }
  .bad-pill { background: var(--danger-muted, rgba(255,80,80,0.15)); color: var(--danger, #e06666); cursor: help; }
</style>
