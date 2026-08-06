// I/O profile card (v0.16, `pg_stat_io`, PG 16+) — mirrors the TUI's
// compact Macro Lens panel: highest-activity backend_type/context rows
// first (the server already orders them that way), rates + hit ratio,
// dashes where track_io_timing is off. Absent entirely (no card) on PG < 16
// or before the first slow collection — see `renderIoStats` in vitals.ts.

import type { IoStatRow } from "./types.ts";

/** Cap on rows shown in the compact card — mirrors the TUI's `IO_ROWS_SHOWN`. */
export const IO_ROWS_SHOWN = 4;

function ratePerSec(v: number | null): string {
  return v === null ? "--" : `${v.toFixed(1)}/s`;
}

function avgMs(v: number | null): string {
  return v === null ? "--" : `${v.toFixed(2)}ms`;
}

function hitRatioPct(v: number | null): string {
  return v === null ? "--" : `${(v * 100).toFixed(0)}%`;
}

/** One formatted line per row, e.g. `client backend/normal: rd 1380.0/s
 * (0.18ms) · wr 58.0/s (0.42ms) · hit 95%`. */
export function ioStatLine(row: IoStatRow): string {
  return (
    `${row.backend_type}/${row.context}: ` +
    `rd ${ratePerSec(row.reads_per_sec)} (${avgMs(row.avg_read_ms)}) · ` +
    `wr ${ratePerSec(row.writes_per_sec)} (${avgMs(row.avg_write_ms)}) · ` +
    `hit ${hitRatioPct(row.hit_ratio)}`
  );
}

/** The full set of lines for the card's detail, capped at `IO_ROWS_SHOWN`,
 * or one calm "no I/O activity" line when the collection succeeded but
 * found nothing (the SQL's HAVING filters all-zero rows out). `rows` itself
 * being `null` (PG < 16 / no collection yet) is the caller's job to check —
 * this function assumes it already has data worth rendering. */
export function ioStatLines(rows: IoStatRow[]): string[] {
  if (rows.length === 0) return ["no I/O activity recorded this interval"];
  return rows.slice(0, IO_ROWS_SHOWN).map(ioStatLine);
}
