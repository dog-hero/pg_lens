// Top-waits aggregation + strip rendering: "what is everyone stuck on".
//
// `topWaits` is a TypeScript port of pg_lens_core/src/waits.rs (`top_waits`)
// — same exclusion, same ordering, same tie-break — kept in lockstep the way
// sql.ts mirrors ui/sql.rs. The ranking is derived data: the server ships
// only `DbSnapshot.activity`, each frontend computes the fold itself.

import type { ActivityRow } from "./types";

export interface WaitSummary {
  /** Sessions with a non-null wait_event (the ones actually stuck). */
  waiting: number;
  /** All sessions in the snapshot — waiting/total is the headline ratio. */
  total: number;
  /** [wait_event, count] pairs, most frequent first; ties alphabetical. */
  ranked: Array<[string, number]>;
}

/**
 * Aggregate `wait_event` (already `Type:Event`) across a snapshot's sessions.
 *
 * Sessions with `wait_event === null` are running, not waiting — excluded
 * from the ranking but counted in `total`. Callers must pass the FULL
 * activity set, never the filtered subset: this answers "what is the
 * *server* stuck on", and a display filter must not change that answer.
 */
export function topWaits(activity: ActivityRow[]): WaitSummary {
  const counts = new Map<string, number>();
  let waiting = 0;
  for (const row of activity) {
    if (row.wait_event !== null) {
      waiting += 1;
      counts.set(row.wait_event, (counts.get(row.wait_event) ?? 0) + 1);
    }
  }
  const ranked = [...counts.entries()].sort(
    // Count descending, then name ascending — deterministic between polls
    // (mirrors the Rust BTreeMap + stable-sort combination).
    (a, b) => b[1] - a[1] || a[0].localeCompare(b[0]),
  );
  return { waiting, total: activity.length, ranked };
}

/** At most this many ranked waits render in the strip (mirrors the TUI). */
const TOP_N = 5;

/**
 * Render the strip into `container`: waiting/total ratio plus the top
 * entries, Lock:* tinted red and IO:* yellow (mirrors the TUI's severity
 * colors). Hidden entirely when nothing waits.
 */
export function renderWaits(container: HTMLElement, activity: ActivityRow[]): void {
  const summary = topWaits(activity);
  if (summary.ranked.length === 0) {
    container.hidden = true;
    container.replaceChildren();
    return;
  }
  container.hidden = false;
  const ratio = document.createElement("span");
  ratio.classList.add("waits-ratio");
  ratio.textContent = `${summary.waiting}/${summary.total} waiting`;
  const items = summary.ranked.slice(0, TOP_N).map(([wait, count]) => {
    const item = document.createElement("span");
    item.classList.add("wait-item");
    if (wait.startsWith("Lock:")) item.classList.add("wait-lock");
    else if (wait.startsWith("IO:")) item.classList.add("wait-io");
    item.textContent = `${wait} ×${count}`;
    return item;
  });
  container.replaceChildren(ratio, ...items);
}
