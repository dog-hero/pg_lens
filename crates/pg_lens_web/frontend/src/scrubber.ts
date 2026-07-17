// History time-scrubber (v0.14, web-only): turns the TPS/sessions uPlot
// chart into an incident-review tool — hover resolves a live readout of
// "what did the vitals look like at that moment", click pins it. Pure
// index/timestamp/formatting logic lives here (DOM-free, node:test-able);
// chart.ts wires uPlot's cursor hooks to it and main.ts owns the pin state
// + readout DOM.

import type { HistoryPoint, SnapshotHistory } from "./types.ts";
import { lockCapacitySeverity } from "./lock_capacity.ts";
import { humanCount } from "./format.ts";

/** One moment's readout, plucked straight out of a `HistoryPoint` — same
 * shape the chart's series already carries, just named for display. */
export interface ReadoutPoint {
  epochMs: number;
  tps: number;
  activeSessions: number;
  connectionsTotal: number;
  /** 0..100, null on points predating v0.14's widening or mid-restart. */
  cacheHitPct: number | null;
  /** 0..100, null when the lock-capacity gauge was unavailable that tick. */
  lockPressurePct: number | null;
  oldestXidAge: number | null;
}

export function toReadout(p: HistoryPoint): ReadoutPoint {
  return {
    epochMs: p.epoch_ms,
    tps: p.tps,
    activeSessions: p.active_sessions,
    connectionsTotal: p.connections_total,
    cacheHitPct: p.cache_hit_pct,
    lockPressurePct: p.lock_pressure_pct,
    oldestXidAge: p.oldest_xid_age,
  };
}

/**
 * Resolves a uPlot cursor/click index into a readout point. `null` for an
 * out-of-range index — uPlot reports `cursor.idx === null` once the pointer
 * leaves the plotting area, which is exactly "no hover" here too.
 */
export function readoutAtIndex(
  history: SnapshotHistory,
  idx: number | null,
): ReadoutPoint | null {
  if (idx === null || idx < 0 || idx >= history.points.length) return null;
  const point = history.points[idx];
  return point === undefined ? null : toReadout(point);
}

/**
 * Re-resolves a PINNED moment (identified by its timestamp, not its index —
 * the ring buffer shifts under an incoming SSE snapshot, so an index alone
 * goes stale) against a refreshed `history`. `null` means the moment has
 * aged out of the 1h ring (its timestamp now precedes the oldest surviving
 * point) — the caller should unpin gracefully rather than show stale data.
 * Points are ascending by `epoch_ms` ([`SnapshotHistory`]'s documented
 * oldest→newest order), so this is a binary search for the nearest sample.
 */
export function resolvePinnedIndex(
  history: SnapshotHistory,
  pinnedEpochMs: number,
): number | null {
  const points = history.points;
  const oldest = points[0];
  if (oldest === undefined) return null;
  if (pinnedEpochMs < oldest.epoch_ms) return null; // aged out

  let lo = 0;
  let hi = points.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const midPoint = points[mid];
    if (midPoint !== undefined && midPoint.epoch_ms < pinnedEpochMs) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  // `lo` is the first index with epoch_ms >= pinnedEpochMs; the nearest
  // neighbor might be the one just before it.
  const cur = points[lo];
  const prev = lo > 0 ? points[lo - 1] : undefined;
  if (prev !== undefined && cur !== undefined) {
    const prevDelta = Math.abs(prev.epoch_ms - pinnedEpochMs);
    const curDelta = Math.abs(cur.epoch_ms - pinnedEpochMs);
    if (prevDelta <= curDelta) return lo - 1;
  }
  return lo;
}

export type ReadoutSeverity = "" | "warn" | "bad";

/** Mirrors vitals.ts's inline `cache_hit_ratio < 0.9 ? "warn"` rule
 * (expressed here in the 0..100 percentage the history series stores). */
export function cacheHitReadoutSeverity(pct: number | null): ReadoutSeverity {
  if (pct === null) return "";
  return pct < 90 ? "warn" : "";
}

/** Reuses lock_capacity.ts's fraction thresholds — `lockPressurePct` is the
 * same `used_fraction` expressed as 0..100 rather than 0..1. */
export function lockPressureReadoutSeverity(pct: number | null): ReadoutSeverity {
  if (pct === null) return "";
  return lockCapacitySeverity(pct / 100);
}

/** `"14:32:08"` — the readout's timestamp, local time (matches uPlot's own
 * axis labels, which are also local). */
export function formatReadoutTime(epochMs: number): string {
  return new Date(epochMs).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** `"3m ago"` / `"live"` — how stale a pinned readout is vs "now", for the
 * subtle "(pinned Xm ago)" hint. Sub-minute deltas read as "just now". */
export function formatPinAge(epochMs: number, nowEpochMs: number): string {
  const deltaSecs = Math.max(0, Math.round((nowEpochMs - epochMs) / 1000));
  if (deltaSecs < 60) return "just now";
  const mins = Math.round(deltaSecs / 60);
  return `${humanCount(mins)}m ago`;
}
