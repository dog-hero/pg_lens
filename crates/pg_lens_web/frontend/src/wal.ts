// WAL generation rate (v0.16, `pg_stat_wal`, PG 14+ only) — mirrors the
// TUI's shared `ui::replication::{wal_buffers_full_severity,
// wal_generation_line}`: same severity rule, same one-line summary text,
// reused by both the Macro dashboard's checkpointer card and the
// Replication Lens.

import type { WalStats } from "./types.ts";
import { humanBytes } from "./format.ts";

export type Severity = "" | "warn";

/**
 * Yellow only when `wal_buffers_full` is ACTIVELY CLIMBING this tick (a real
 * `wal_buffers` sizing signal, not just a nonzero cumulative count from
 * history) — never escalates to red, a tuning nudge rather than an incident,
 * mirroring `checkpointPressureSeverity`.
 */
export function walBuffersFullSeverity(wal: WalStats): Severity {
  return wal.wal_buffers_full_delta !== null && wal.wal_buffers_full_delta > 0
    ? "warn"
    : "";
}

/**
 * One-line WAL generation summary: bytes/s, records/s, and the
 * `wal_buffers_full` pressure signal — dashes for the rates before the
 * first delta window this session (never a misleading `0/s`).
 */
export function walGenerationText(wal: WalStats): string {
  const bytesRate =
    wal.wal_bytes_per_sec !== null
      ? `${humanBytes(Math.max(0, wal.wal_bytes_per_sec))}/s`
      : "--";
  const recordsRate =
    wal.wal_records_per_sec !== null
      ? `${wal.wal_records_per_sec.toFixed(0)} rec/s`
      : "-- rec/s";
  const buffersFull =
    wal.wal_buffers_full_delta !== null && wal.wal_buffers_full_delta > 0
      ? `${wal.wal_buffers_full} (+${wal.wal_buffers_full_delta} this tick)`
      : `${wal.wal_buffers_full}`;
  return `WAL generation: ${bytesRate} · ${recordsRate}  buffers_full: ${buffersFull}`;
}
