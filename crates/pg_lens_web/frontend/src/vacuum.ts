// Vacuum health / XID wraparound severity (F2) — shared by the vitals
// warning chip and the Schema Lens's vacuum section, mirroring the TUI's
// ui/vacuum.rs (one set of thresholds, defined once).
//
// Thresholds: yellow past autovacuum_freeze_max_age's default (200M — the
// point autovacuum starts forcing freeze scans specifically to fight
// wraparound), red past 500M (a quarter of the way to the ~2.1 billion
// forced-shutdown ceiling).

export const WARN_AGE_XIDS = 200_000_000;
export const BAD_AGE_XIDS = 500_000_000;

export type Severity = "" | "warn" | "bad";

export function ageSeverity(ageXids: number): Severity {
  if (ageXids > BAD_AGE_XIDS) return "bad";
  if (ageXids > WARN_AGE_XIDS) return "warn";
  return "";
}
