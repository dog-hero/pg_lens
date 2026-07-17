// Prepared-transaction (orphaned 2PC) watch (v0.9).
//
// TypeScript port of pg_lens_core/src/prepared_xacts.rs — same thresholds,
// kept in lockstep the way xact_age.ts mirrors xact_age.rs / waits.ts
// mirrors waits.rs. Pure derivation over `DbSnapshot.prepared_xacts`, no
// new fetch.

/** Yellow past this age (5 minutes) — a 2PC coordinator that hasn't
 * resolved a prepared transaction in 5 minutes is almost certainly gone. */
export const WARN_AGE_SECS = 300;
/** Red past this age (1 hour) — no legitimate 2PC coordinator takes this
 * long; this is an orphan holding locks and pinning wraparound. */
export const BAD_AGE_SECS = 3_600;

export type PreparedXactSeverity = "" | "warn" | "bad";

/** Severity of one prepared transaction's age. */
export function preparedXactSeverity(ageSeconds: number): PreparedXactSeverity {
  if (ageSeconds > BAD_AGE_SECS) return "bad";
  if (ageSeconds > WARN_AGE_SECS) return "warn";
  return "";
}
