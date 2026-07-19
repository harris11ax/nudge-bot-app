<script>
  // Bulk Upload screen (PLAN-bulk-upload.md §6, P5). Reached from the Tasks page.
  // Upload a .csv/.xlsx (or paste CSV) → server validate_bulk_upload splits rows
  // into new-unique (valid table) vs ignored-duplicate (report block), both fully
  // editable in place. The dedup rule (§5) is the server's alone: every cell edit
  // re-serializes ALL rows back to a canonical 10-col CSV and re-calls the server,
  // so a row moves bidirectionally between the two tables purely by the mechanical
  // title/deadline rule — never a manual toggle. Confirm resolves Group→Project
  // and inserts every included row in one transaction (confirm_bulk_upload).
  import { validateBulkUpload } from "./api.js";
  import { bulkUploadConfirm } from "./store.svelte.js";

  let { onBack } = $props();

  const HEADER =
    "project_group,project,title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode";
  const CELL_FIELDS = HEADER.split(",");

  const LLM_PROMPT = `Convert my task list below into CSV for bulk upload into a task app.
Return ONLY the CSV — no prose, no code fences — with exactly this header row first:

${HEADER}

Rules:
- project_group: optional; names/creates a Project Group to file the task under.
- project: optional; names/creates a Project inside that Group. Requires project_group.
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

  // Unified row model: valid rows and ignored rows are the SAME shape; the
  // `ignored` flag + `reason` decide which table renders them.
  let rows = $state(/** @type {Array} */ ([]));
  let err = $state("");
  let confirming = $state(false);
  let confirmMsg = $state("");
  let copied = $state(false);
  let pasteText = $state("");

  const validRows = $derived(rows.filter((r) => !r.ignored));
  const ignoredRows = $derived(rows.filter((r) => r.ignored));
  const readyCount = $derived(validRows.filter((r) => r.valid && r.include).length);

  /** Build the editable-cell map for a row from a DTO's raw (field,value) pairs. */
  function cellsFromDto(dto) {
    const cells = {};
    for (const f of CELL_FIELDS) cells[f] = "";
    for (const [k, v] of dto.raw) cells[k] = v;
    return cells;
  }

  /** Stable content signature so a manual include-toggle survives a re-validate. */
  function sig(cells) {
    return CELL_FIELDS.map((f) => cells[f] ?? "").join("");
  }

  function mkRow(dto, ignored, reason, excluded) {
    const cells = cellsFromDto(dto);
    return {
      cells,
      form: dto.form,
      valid: dto.valid,
      errors: dto.errors,
      ignored,
      reason,
      // Ignored rows can't be included; valid rows default on unless the user
      // had explicitly excluded this exact content before the edit.
      include: dto.valid && !ignored && !excluded.has(sig(cells)),
    };
  }

  /** Escape a single CSV cell (quote if it holds a comma, quote, or newline). */
  function csvCell(v) {
    const s = v ?? "";
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  }

  /** Serialize the current rows (valid first, then ignored) to a 10-col CSV blob. */
  function allToCsv() {
    const lines = rows.map((r) => CELL_FIELDS.map((f) => csvCell(r.cells[f])).join(","));
    return [HEADER, ...lines].join("\n");
  }

  /** Rebuild both tables from a validation response, preserving include state. */
  function applyValidation(dto, excluded) {
    rows = [
      ...dto.new_rows.map((d) => mkRow(d, false, "", excluded)),
      ...dto.ignored_rows.map((ig) => mkRow(ig.row, true, ig.reason, excluded)),
    ];
  }

  /** Initial validation of a freshly loaded blob. `bytes` = Array<number>. */
  async function validateBlob(bytes, ext) {
    err = "";
    confirmMsg = "";
    try {
      const dto = await validateBulkUpload(bytes, ext);
      applyValidation(dto, new Set());
      if (!rows.length) err = "No data rows found in that file.";
    } catch (e) {
      err = String(e);
    }
  }

  /**
   * Re-run the server dedup over ALL current rows after any inline edit. Rows may
   * move between the valid table and the report block by the §5 rule alone.
   */
  async function revalidateAll() {
    err = "";
    const excluded = new Set(rows.filter((r) => !r.include).map((r) => sig(r.cells)));
    try {
      const bytes = Array.from(new TextEncoder().encode(allToCsv()));
      const dto = await validateBulkUpload(bytes, "csv");
      applyValidation(dto, excluded);
    } catch (e) {
      err = String(e);
    }
  }

  function onFile(e) {
    const file = e.target.files?.[0];
    if (!file) return;
    const ext = (file.name.split(".").pop() ?? "").toLowerCase();
    const reader = new FileReader();
    reader.onload = () => {
      const buf = reader.result;
      const bytes =
        typeof buf === "string"
          ? Array.from(new TextEncoder().encode(buf))
          : Array.from(new Uint8Array(buf));
      validateBlob(bytes, ext || "csv");
    };
    // xlsx/xlsm need the raw bytes; csv can go either way — read as buffer for both.
    reader.readAsArrayBuffer(file);
    e.target.value = ""; // allow re-picking the same file
  }

  function validatePaste() {
    if (pasteText.trim()) validateBlob(Array.from(new TextEncoder().encode(pasteText)), "csv");
  }

  function downloadTemplate() {
    const blob = new Blob([`${HEADER}\n`], { type: "text/csv" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "bulk-upload-template.csv";
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

  async function doConfirm() {
    err = "";
    confirmMsg = "";
    confirming = true;
    try {
      const payload = validRows
        .filter((r) => r.valid && r.include)
        .map((r) => ({
          form: r.form,
          project_group: r.cells.project_group ?? "",
          project: r.cells.project ?? "",
        }));
      const n = await bulkUploadConfirm(payload);
      confirmMsg = `Uploaded ${n} task${n === 1 ? "" : "s"}.`;
      rows = [];
      pasteText = "";
    } catch (e) {
      err = String(e);
    } finally {
      confirming = false;
    }
  }
</script>

<header class="head">
  <button class="back" onclick={onBack} title="Back to Tasks">← Tasks</button>
  <h1>Bulk Upload</h1>
</header>

<section class="card">
  <h2>1 · Prepare</h2>
  <p class="hint">
    Turn a meeting transcript or notes into a spreadsheet in Claude or Gemini using the header below,
    then upload the <code>.csv</code> or <code>.xlsx</code>. Two leading columns file each task under a
    Project Group → Project (created on confirm). Extra columns are ignored; a wrong header shows rows
    as invalid.
  </p>
  <p class="hint"><code>{HEADER}</code></p>
  <div class="btnrow">
    <button onclick={downloadTemplate}>Download blank template.csv</button>
    <button onclick={copyPrompt}>{copied ? "Copied ✓" : "Copy LLM prompt"}</button>
  </div>
</section>

<section class="card">
  <h2>2 · Load a file</h2>
  <div class="btnrow">
    <label class="filebtn">
      Choose file…
      <input type="file" accept=".csv,.xlsx,.xlsm" onchange={onFile} />
    </label>
    <span class="hint">or paste CSV text:</span>
  </div>
  <textarea rows="4" placeholder={HEADER} bind:value={pasteText}></textarea>
  <div class="btnrow">
    <button onclick={validatePaste} disabled={!pasteText.trim()}>Validate pasted CSV</button>
  </div>
  {#if err}<p class="error">{err}</p>{/if}
  {#if confirmMsg}<p class="ok">{confirmMsg}</p>{/if}
</section>

{#if ignoredRows.length}
  <section class="card report">
    <div class="filter-head">
      <h2>Ignored duplicates</h2>
      <span class="pill bad-pill">{ignoredRows.length} ignored</span>
    </div>
    <p class="hint">
      Each row is an exact duplicate — its title AND deadline both match an existing task. Edit the title
      or the deadline so the pair is unique and the row jumps up to the upload table.
    </p>
    <div class="tablewrap">
      <table class="filter">
        <thead>
          <tr>
            <th>Reason</th>
            <th>Group</th>
            <th>Project</th>
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
          {#each rows as r, i (i)}
            {#if r.ignored}
              <tr class="dup">
                <td class="reason" title={r.reason}>{r.reason}</td>
                <td><input class="s" bind:value={r.cells.project_group} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.project} onchange={revalidateAll} /></td>
                <td><input bind:value={r.cells.title} onchange={revalidateAll} /></td>
                <td><input class="w" bind:value={r.cells.deadline} placeholder="YYYY-MM-DD" onchange={revalidateAll} /></td>
                <td><input class="xs" bind:value={r.cells.time_of_day} placeholder="HH:MM" onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.recur} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.task_type} onchange={revalidateAll} /></td>
                <td><input class="xs" bind:value={r.cells.estimate_minutes} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.mode} onchange={revalidateAll} /></td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  </section>
{/if}

{#if validRows.length}
  <section class="card">
    <div class="filter-head">
      <h2>New tasks</h2>
      <span class="hint">{readyCount} of {validRows.length} row(s) ready</span>
    </div>
    <div class="tablewrap">
      <table class="filter">
        <thead>
          <tr>
            <th></th>
            <th>Status</th>
            <th>Group</th>
            <th>Project</th>
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
          {#each rows as r, i (i)}
            {#if !r.ignored}
              <tr class:invalid={!r.valid}>
                <td>
                  <input
                    type="checkbox"
                    bind:checked={r.include}
                    disabled={!r.valid}
                    title={r.valid ? "Include in upload" : "Fix errors to include"}
                  />
                </td>
                <td>
                  {#if r.valid}
                    <span class="pill ok-pill">ready</span>
                  {:else}
                    <span class="pill bad-pill" title={r.errors.join("; ")}>needs fix</span>
                  {/if}
                </td>
                <td><input class="s" bind:value={r.cells.project_group} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.project} onchange={revalidateAll} /></td>
                <td><input bind:value={r.cells.title} onchange={revalidateAll} /></td>
                <td><input class="w" bind:value={r.cells.deadline} placeholder="YYYY-MM-DD" onchange={revalidateAll} /></td>
                <td><input class="xs" bind:value={r.cells.time_of_day} placeholder="HH:MM" onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.recur} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.task_type} onchange={revalidateAll} /></td>
                <td><input class="xs" bind:value={r.cells.estimate_minutes} onchange={revalidateAll} /></td>
                <td><input class="s" bind:value={r.cells.mode} onchange={revalidateAll} /></td>
              </tr>
              {#if !r.valid && r.errors.length}
                <tr class="errrow"><td></td><td colspan="10" class="errcell">{r.errors.join("; ")}</td></tr>
              {/if}
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
    <div class="btnrow">
      <button class="primary" onclick={doConfirm} disabled={confirming || readyCount === 0}>
        {confirming ? "Uploading…" : `Confirm ${readyCount} task(s)`}
      </button>
    </div>
  </section>
{/if}

<style>
  .head { display: flex; align-items: center; gap: 12px; }
  .back {
    padding: 4px 10px; border: 1px solid var(--border, #333); border-radius: 6px;
    cursor: pointer; font-size: 13px; background: transparent; color: inherit;
  }
  .back:hover { filter: brightness(1.15); }
  .btnrow { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 8px; }
  textarea { width: 100%; box-sizing: border-box; font-family: monospace; font-size: 12px; }
  .filebtn {
    display: inline-block; padding: 6px 12px; border: 1px solid var(--border, #333);
    border-radius: 6px; cursor: pointer; font-size: 13px;
  }
  .filebtn:hover { filter: brightness(1.1); }
  .filebtn input { display: none; }
  .report { border-left: 3px solid var(--danger, #e06666); }
  .filter-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
  .filter-head h2 { margin: 0; }
  .tablewrap { overflow-x: auto; margin-top: 8px; }
  table.filter { border-collapse: collapse; width: 100%; font-size: 13px; }
  table.filter th { text-align: left; padding: 4px 6px; color: var(--fg-muted); font-weight: 600; }
  table.filter td { padding: 3px 6px; border-top: 1px solid var(--border, #2a2a2a); }
  tr.invalid td { background: var(--danger-muted, rgba(255,80,80,0.06)); }
  tr.dup td { background: var(--danger-muted, rgba(255,80,80,0.05)); }
  tr.errrow td { border-top: none; }
  .errcell { color: var(--danger); font-size: 12px; padding-top: 0; padding-bottom: 6px; }
  .reason { color: var(--danger, #e06666); font-size: 12px; white-space: nowrap; max-width: 14rem; overflow: hidden; text-overflow: ellipsis; }
  table.filter input { width: 9rem; box-sizing: border-box; font-size: 12px; padding: 2px 4px; }
  table.filter input.s { width: 6rem; }
  table.filter input.xs { width: 4rem; }
  table.filter input.w { width: 11rem; }
  .pill { font-size: 11px; padding: 1px 8px; border-radius: 1rem; white-space: nowrap; }
  .ok-pill { background: var(--ok-muted, rgba(80,200,120,0.15)); color: var(--ok, #4caf70); }
  .bad-pill { background: var(--danger-muted, rgba(255,80,80,0.15)); color: var(--danger, #e06666); cursor: help; }
</style>
