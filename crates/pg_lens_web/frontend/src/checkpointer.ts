// Checkpointer / bgwriter card (F4) — mirrors the TUI's Macro Lens
// "Checkpoints / writer" panel: same rate/ratio derivation contract as the
// core (`derive_checkpointer_stats`), same session-window pressure rule.

import type { CheckpointerStats } from "./types.ts";
import { humanMs } from "./format.ts";

export type Severity = "" | "warn";

/**
 * A high requested-checkpoint share over the poller SESSION window (not
 * per-tick — checkpoints are rare) means `max_wal_size` is likely too
 * small. Yellow only — a tuning signal, not an incident. `null` (no
 * checkpoint yet this session) renders calm, mirroring the TUI's
 * `checkpoint_pressure_severity`.
 */
export function checkpointPressureSeverity(
  ratio: number | null,
): Severity {
  return ratio !== null && ratio > 0.5 ? "warn" : "";
}

function ratePerSec(v: number | null): string {
  return v === null ? "--" : `${v.toFixed(1)}/s`;
}

/** Backend-issued writes: absent for a real reason on PG 17+ (moved to
 * pg_stat_io) — say so instead of a bare dash. */
function backendRateText(cp: CheckpointerStats): string {
  if (cp.buffers_backend === null) return "n/a (17+)";
  return ratePerSec(cp.buffers_backend_per_sec);
}

export interface CheckpointerCard {
  perMin: string;
  pressure: string;
  buffersPerSec: string;
  avgWriteSync: string;
  severity: Severity;
}

export function checkpointerCard(cp: CheckpointerStats): CheckpointerCard {
  const severity = checkpointPressureSeverity(cp.requested_ratio_session);
  const perMin =
    cp.checkpoints_per_min_timed !== null && cp.checkpoints_per_min_req !== null
      ? `${cp.checkpoints_per_min_timed.toFixed(2)} timed / ${cp.checkpoints_per_min_req.toFixed(2)} req /min`
      : "-- timed / -- req /min";
  const pressure =
    cp.requested_ratio_session !== null
      ? `${(cp.requested_ratio_session * 100).toFixed(0)}% requested (session)`
      : "-- (no checkpoint yet this session)";
  const buffersPerSec = `chkpt ${ratePerSec(cp.buffers_checkpoint_per_sec)} · bgwriter ${ratePerSec(
    cp.buffers_clean_per_sec,
  )} · backend ${backendRateText(cp)}`;
  const avgWriteSync = `${
    cp.avg_checkpoint_write_ms !== null ? humanMs(cp.avg_checkpoint_write_ms) : "--"
  } / ${cp.avg_checkpoint_sync_ms !== null ? humanMs(cp.avg_checkpoint_sync_ms) : "--"}`;
  return { perMin, pressure, buffersPerSec, avgWriteSync, severity };
}
