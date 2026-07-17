// Lock-table pressure gauge (v0.11).
//
// TypeScript port of pg_lens_core/src/lock_capacity.rs — same thresholds,
// kept in lockstep the way prepared_xacts.ts mirrors prepared_xacts.rs.
// `used_fraction`/`capacity_slots` themselves are computed server-side
// (`lock_capacity::compute`) and travel as-is on `DbSnapshot.lock_capacity`
// — only the severity tiering is re-derived here.

/** Yellow past this fraction of the lock table's capacity — comfortably
 * before exhaustion, but worth watching. */
export const WARN_FRACTION = 0.6;
/** Red past this fraction — close enough to "out of shared memory" that an
 * operator should act now. */
export const BAD_FRACTION = 0.85;

export type LockCapacitySeverity = "" | "warn" | "bad";

/** Severity of one used-fraction reading. */
export function lockCapacitySeverity(usedFraction: number): LockCapacitySeverity {
  if (usedFraction > BAD_FRACTION) return "bad";
  if (usedFraction > WARN_FRACTION) return "warn";
  return "";
}
