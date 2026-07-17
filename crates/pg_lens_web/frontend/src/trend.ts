// Trend arrows for the Macro Lens vitals cards (v0.14).
//
// TypeScript port of pg_lens_core/src/history.rs's `trend()` +
// `SnapshotHistory::sample_for_trend` — same deadband/lookback constants,
// kept in lockstep the way lock_capacity.ts mirrors lock_capacity.rs.

import type { HistoryPoint, SnapshotHistory } from "./types";

export type Trend = "up" | "down" | "flat";

/** Relative deadband: changes under 5% of the baseline are noise. */
export const TREND_DEADBAND = 0.05;

/** ~5 minutes at the 2s default poll interval. */
export const TREND_LOOKBACK_TICKS = 150;

/**
 * Compares `now` against `then` with a *relative* deadband — mirrors the
 * Rust core exactly (including the zero-baseline fallback to an absolute
 * deadband).
 */
export function trend(now: number, then: number, deadband: number): Trend {
  const db = Math.abs(deadband);
  const threshold = Math.abs(then) > Number.EPSILON ? Math.abs(then) * db : db;
  const delta = now - then;
  if (Math.abs(delta) < threshold) return "flat";
  return delta > 0 ? "up" : "down";
}

/**
 * The sample ~`lookbackTicks` before the latest one, clamped to the oldest
 * available point — mirrors `SnapshotHistory::sample_for_trend`. `null` on
 * an empty ring.
 */
export function sampleForTrend(
  history: SnapshotHistory,
  lookbackTicks: number,
): HistoryPoint | null {
  const points = history.points;
  if (points.length === 0) return null;
  const idx = Math.max(0, points.length - 1 - lookbackTicks);
  return points[idx] ?? null;
}

/**
 * `now` vs the sample ~5 minutes back, with the shared deadband. `then =
 * null` (no baseline yet — a fresh session, or a history point predating
 * this metric) renders `"flat"`, same rule as the TUI's `card_trend`.
 */
export function cardTrend(now: number, then: number | null): Trend {
  return then === null ? "flat" : trend(now, then, TREND_DEADBAND);
}

/** `↑`/`↓`/`→`. */
export function trendGlyph(t: Trend): string {
  switch (t) {
    case "up":
      return "↑";
    case "down":
      return "↓";
    case "flat":
      return "→";
  }
}

/**
 * CSS class for the trend arrow: `""` (dim/neutral) unless the direction is
 * the concerning one for this metric (`upIsBad`), which gets `"warn"` — same
 * subtle-tint rule as the TUI's `trend_color`.
 */
export function trendTone(t: Trend, upIsBad: boolean): "" | "warn" {
  if (t === "up" && upIsBad) return "warn";
  if (t === "down" && !upIsBad) return "warn";
  return "";
}

/**
 * The trend arrow's `title` tooltip, e.g. `"+12% vs 5 min ago"`. `now`/`then`
 * are in the same unit as displayed on the card (percentage points for
 * cache-hit/lock-pressure, raw connection counts); `unit` is appended
 * verbatim after the signed delta (`"%"` or `""`).
 */
export function trendTitle(now: number, then: number | null, unit: string): string {
  if (then === null) return "no 5-minute baseline yet";
  const delta = now - then;
  const sign = delta > 0 ? "+" : delta < 0 ? "" : "±";
  return `${sign}${delta.toFixed(1)}${unit} vs 5 min ago`;
}
