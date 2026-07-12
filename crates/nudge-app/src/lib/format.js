// Pure display helpers shared across Planner / Triggers / Sidebar. No Svelte, no
// Tauri — just task-row formatting.

const DAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/** Minutes-since-midnight → "HH:MM" (or "" when null). */
export function hhmm(minutes) {
  if (minutes == null) return "";
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

/** Recur spec → human label. "once" → "one-time"; day list stays as-is. */
export function recurLabel(spec) {
  if (!spec || spec === "once") return "one-time";
  const days = spec.split(",").filter(Boolean);
  if (days.length === 7) return "daily";
  if (days.length === 5 && DAYS.slice(0, 5).every((d) => days.includes(d)))
    return "weekdays";
  return days.join(", ");
}

/** Unix-seconds deadline → short local date/time, or "" when null. */
export function deadlineLabel(unix) {
  if (unix == null) return "";
  return new Date(unix * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Coarse countdown to a unix deadline: "overdue", "3h", "2d". "" when null. */
export function countdown(unix, nowMs = Date.now()) {
  if (unix == null) return "";
  const diff = unix * 1000 - nowMs;
  if (diff <= 0) return "overdue";
  const mins = Math.round(diff / 60000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.round(mins / 60);
  if (hrs < 48) return `${hrs}h`;
  return `${Math.round(hrs / 24)}d`;
}

/** True when a deadline is in the past. */
export const isMissed = (unix, nowMs = Date.now()) =>
  unix != null && unix * 1000 < nowMs;
