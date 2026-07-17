// Idle connection / connection-age census (v0.11).
//
// TypeScript port of pg_lens_core/src/idle_sessions.rs — same thresholds,
// kept in lockstep the way prepared_xacts.ts mirrors prepared_xacts.rs.
// Pure derivation over `DbSnapshot.idle_sessions`, no new fetch: the census
// is rendered as a collapsible list under the activity table (mirrors the
// TUI's `I` toggle), never crowding the active-session table with more
// columns.

import type { IdleSessionRow } from "./types";
import { humanDuration } from "./format.ts";

/** Yellow past this age (30 minutes) — longer than almost any legitimate
 * connection-pool idle timeout, worth a second look. */
export const WARN_AGE_SECS = 1_800;
/** Red past this age (4 hours) — no ordinary pool keeps a connection idle
 * this long; this is the classic "leaked connection" suspect. */
export const BAD_AGE_SECS = 14_400;

export type IdleSessionSeverity = "" | "warn" | "bad";

/** Severity of one idle session's age. */
export function idleSessionSeverity(idleAgeSecs: number): IdleSessionSeverity {
  if (idleAgeSecs > BAD_AGE_SECS) return "bad";
  if (idleAgeSecs > WARN_AGE_SECS) return "warn";
  return "";
}

/** The oldest idle session in a set — the prime pool-exhaustion suspect.
 * `undefined` when `rows` is empty. */
export function oldestIdleSession(rows: IdleSessionRow[]): IdleSessionRow | undefined {
  return rows.reduce<IdleSessionRow | undefined>((oldest, row) => {
    if (oldest === undefined || row.idle_age_secs > oldest.idle_age_secs) return row;
    return oldest;
  }, undefined);
}

/**
 * The complete idle census, rendered inside a `<details>` element under the
 * activity table — mirrors the TUI's `I` toggle: a count + oldest-suspect
 * summary line, then one severity-colored row per idle session (PID, age,
 * user, db, app, client). `null` (best-effort collection failed this tick)
 * and an empty array both collapse the section — there is nothing useful to
 * show either way, and `null` is not itself an error worth a banner.
 */
export function renderIdleSessions(
  details: HTMLDetailsElement,
  summaryEl: HTMLElement,
  list: HTMLUListElement,
  rows: IdleSessionRow[] | null,
): void {
  if (rows === null || rows.length === 0) {
    details.hidden = true;
    list.replaceChildren();
    return;
  }
  details.hidden = false;
  const oldest = oldestIdleSession(rows);
  const oldestText = oldest !== undefined ? ` (oldest ${humanDuration(oldest.idle_age_secs)})` : "";
  summaryEl.textContent = `Idle connections (${rows.length})${oldestText}`;
  const items = rows.map((row) => {
    const sev = idleSessionSeverity(row.idle_age_secs);
    const li = document.createElement("li");
    li.className = sev ? `vacuum-table ${sev}` : "vacuum-table";
    li.textContent =
      `pid ${row.pid} — ${humanDuration(row.idle_age_secs)} idle · ` +
      `${row.username}@${row.database} · ${row.application_name || "(no app name)"} · ` +
      row.client;
    return li;
  });
  list.replaceChildren(...items);
}
