// Idle-in-transaction / transaction-age hunter (v0.9).
//
// TypeScript port of pg_lens_core/src/xact_age.rs — same thresholds, same
// idle-in-transaction escalation, kept in lockstep the way waits.ts mirrors
// waits.rs. Pure derivation over `DbSnapshot.activity`, no new fetch.

import type { ActivityRow } from "./types";
import { humanDuration } from "./format.ts";

/** Yellow past this age for a normally-running transaction (5 minutes). */
export const XACT_AGE_WARN_SECS = 300;
/** Red past this age for a normally-running transaction (30 minutes). */
export const XACT_AGE_BAD_SECS = 1_800;
/**
 * `idle in transaction` sessions get HALF these thresholds: they hold locks
 * and pin the wraparound horizon while doing nothing, so the same age is
 * strictly worse than an actively-running transaction of equal age.
 */
export const IDLE_IN_XACT_AGE_WARN_SECS = XACT_AGE_WARN_SECS / 2;
export const IDLE_IN_XACT_AGE_BAD_SECS = XACT_AGE_BAD_SECS / 2;

export type XactAgeSeverity = "ok" | "warn" | "bad";

function isIdleInTransaction(state: string): boolean {
  return state === "idle in transaction" || state === "idle in transaction (aborted)";
}

/** Severity of one transaction age, escalated for idle-in-transaction sessions. */
export function xactAgeSeverity(ageSecs: number, state: string): XactAgeSeverity {
  const [warn, bad] = isIdleInTransaction(state)
    ? [IDLE_IN_XACT_AGE_WARN_SECS, IDLE_IN_XACT_AGE_BAD_SECS]
    : [XACT_AGE_WARN_SECS, XACT_AGE_BAD_SECS];
  if (ageSecs > bad) return "bad";
  if (ageSecs > warn) return "warn";
  return "ok";
}

export interface OldestXact {
  row: ActivityRow;
  severity: XactAgeSeverity;
}

/**
 * Find the session with the largest `xact_age_secs` (rows with no open
 * transaction are skipped, never treated as age 0) — the single
 * transaction most likely to be pinning `datfrozenxid` or holding locks
 * other sessions are queued on.
 */
export function oldestOpenXact(activity: ActivityRow[]): OldestXact | null {
  let best: { row: ActivityRow; age: number } | null = null;
  for (const row of activity) {
    if (row.xact_age_secs === null) continue;
    if (best === null || row.xact_age_secs > best.age) {
      best = { row, age: row.xact_age_secs };
    }
  }
  if (best === null) return null;
  return { row: best.row, severity: xactAgeSeverity(best.age, best.row.state) };
}

/**
 * Render the "oldest open transaction" headline into `container` (mirrors
 * the TUI's one-liner: age, pid, state). Hidden whenever there is no open
 * transaction at all, OR the oldest one is still `ok` severity — a calm
 * snapshot stays quiet, same contract as the waits strip.
 */
export function renderOldestXact(
  container: HTMLElement,
  ageEl: HTMLElement,
  metaEl: HTMLElement,
  stateEl: HTMLElement,
  activity: ActivityRow[],
): void {
  const oldest = oldestOpenXact(activity);
  container.classList.remove("severity-warn", "severity-bad");
  if (oldest === null || oldest.severity === "ok") {
    container.hidden = true;
    return;
  }
  container.hidden = false;
  container.classList.add(oldest.severity === "bad" ? "severity-bad" : "severity-warn");
  ageEl.textContent = humanDuration(oldest.row.xact_age_secs ?? 0);
  metaEl.textContent = `pid ${oldest.row.pid}`;
  stateEl.textContent = oldest.row.state;
}
