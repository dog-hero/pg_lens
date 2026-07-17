// Human formatting helpers, mirroring the TUI's ui/format.rs conventions.

/** `4m32s`, `1h04m`, `3d 4h` — compact human duration. */
export function humanDuration(totalSecs: number): string {
  if (!Number.isFinite(totalSecs) || totalSecs < 0) return "-";
  if (totalSecs < 1) return `${(totalSecs * 1000).toFixed(0)}ms`;
  const s = Math.floor(totalSecs);
  if (s < 60) return `${totalSecs.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m${String(s % 60).padStart(2, "0")}s`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h${String(m % 60).padStart(2, "0")}m`;
  const d = Math.floor(h / 24);
  return `${d}d ${h % 24}h`;
}

/** `1.2 GB`, `48 MB`, `512 B` — binary-ish human bytes (1024 base). */
export function humanBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 || unit === 0 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

/** Signed human byte delta for growth columns: `+120 MB`, `-3.1 MB` — unlike
 * `humanBytes`, preserves the sign (shrinkage is a valid, meaningful
 * reading, never clamped away). Mirrors the TUI's `human_bytes_signed`. */
export function humanBytesSigned(bytes: number): string {
  if (!Number.isFinite(bytes)) return "-";
  return bytes < 0 ? `-${humanBytes(-bytes)}` : `+${humanBytes(bytes)}`;
}

/** Thousands-separated integer, e.g. `1,254`. */
export function humanCount(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** `4m32s ago`, or `—` when the event never happened (TUI's human_ago). */
export function humanAgo(
  epochSecs: number | null,
  nowEpochSecs: number,
): string {
  if (epochSecs === null || !Number.isFinite(epochSecs)) return "—";
  return `${humanDuration(Math.max(0, nowEpochSecs - epochSecs))} ago`;
}

/** `95.7%` from a 0..1 ratio. */
export function humanPercent(ratio: number): string {
  if (!Number.isFinite(ratio)) return "-";
  return `${(ratio * 100).toFixed(1)}%`;
}

/** `189442.7ms` → `3m09s`-style: source values are already milliseconds
 * (pg_stat_statements, checkpoint write/sync time). */
export function humanMs(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0ms";
  if (ms < 1) return `${ms.toFixed(2)}ms`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return humanDuration(ms / 1000);
}
