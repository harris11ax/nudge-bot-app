<script>
  import { quickAdd, createTask } from "./store.svelte.js";

  // Quick-add: `text @ time [recur]`, parsed by nudge-core via the backend so the
  // grammar has exactly one implementation.
  let line = $state("");
  let quickErr = $state("");
  let quickOk = $state("");

  async function submitQuick(e) {
    e.preventDefault();
    quickErr = "";
    quickOk = "";
    try {
      await quickAdd(line);
      quickOk = `Added: ${line}`;
      line = "";
    } catch (err) {
      quickErr = String(err);
    }
  }

  // Fallback structured form for anything quick-add can't express (deadline,
  // description, type, mode override).
  let form = $state({
    title: "",
    desc: "",
    task_type: "",
    time: "", // HH:MM → minutes on submit
    recur: "once",
    deadline: "", // datetime-local → unix seconds on submit
    mode_override: "",
  });
  let formErr = $state("");
  let formOk = $state("");

  function toMinutes(hhmm) {
    if (!hhmm) return null;
    const [h, m] = hhmm.split(":").map(Number);
    return h * 60 + m;
  }

  async function submitForm(e) {
    e.preventDefault();
    formErr = "";
    formOk = "";
    if (!form.title.trim()) {
      formErr = "Title is required.";
      return;
    }
    const payload = {
      title: form.title.trim(),
      desc: form.desc,
      task_type: form.task_type,
      minutes: toMinutes(form.time),
      recur: form.recur.trim() || "once",
      deadline: form.deadline ? Math.floor(new Date(form.deadline).getTime() / 1000) : null,
      mode_override: form.mode_override || null,
    };
    try {
      await createTask(payload);
      formOk = `Added: ${payload.title}`;
      form = { title: "", desc: "", task_type: "", time: "", recur: "once", deadline: "", mode_override: "" };
    } catch (err) {
      formErr = String(err);
    }
  }
</script>

<header class="head"><h1>Triggers</h1></header>

<section class="card">
  <h2>Quick add</h2>
  <p class="hint">Format: <code>text @ time [recur]</code> — e.g. <code>gym @ 17:30 mon,wed,fri</code></p>
  <form onsubmit={submitQuick} class="quickadd">
    <input placeholder="gym @ 17:30 mon,wed,fri" bind:value={line} />
    <button class="primary" type="submit">Add</button>
  </form>
  {#if quickErr}<p class="error">{quickErr}</p>{/if}
  {#if quickOk}<p class="ok">{quickOk}</p>{/if}
</section>

<section class="card">
  <h2>Full trigger</h2>
  <form onsubmit={submitForm} class="grid">
    <label>Title<input bind:value={form.title} required /></label>
    <label>Type<input bind:value={form.task_type} placeholder="health, work…" /></label>
    <label>Time of day<input type="time" bind:value={form.time} /></label>
    <label>Recur<input bind:value={form.recur} placeholder="once / daily / mon,wed,fri" /></label>
    <label>Deadline<input type="datetime-local" bind:value={form.deadline} /></label>
    <label>Mode
      <select bind:value={form.mode_override}>
        <option value="">Auto (classify at edge)</option>
        <option value="off_task">Force strong (off-task)</option>
        <option value="on_task">Force soft (on-task)</option>
      </select>
    </label>
    <label class="wide">Description<textarea rows="2" bind:value={form.desc}></textarea></label>
    <div class="wide"><button class="primary" type="submit">Create trigger</button></div>
  </form>
  {#if formErr}<p class="error">{formErr}</p>{/if}
  {#if formOk}<p class="ok">{formOk}</p>{/if}
</section>
