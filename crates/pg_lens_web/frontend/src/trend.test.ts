// Unit tests for the trend-arrow math — mirrors pg_lens_core's history.rs
// test suite so both implementations stay in lockstep.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  TREND_DEADBAND,
  cardTrend,
  sampleForTrend,
  trend,
  trendGlyph,
  trendTitle,
  trendTone,
} from "./trend.ts";
import type { HistoryPoint, SnapshotHistory } from "./types.ts";

function point(i: number): HistoryPoint {
  return {
    epoch_ms: i,
    tps: i,
    active_sessions: i,
    connections_total: i,
    cache_hit_pct: i,
    lock_pressure_pct: i,
    oldest_xid_age: i,
  };
}

test("trend is flat within the deadband", () => {
  assert.equal(trend(103, 100, TREND_DEADBAND), "flat");
  assert.equal(trend(50, 50, TREND_DEADBAND), "flat");
});

test("trend is up/down outside the deadband", () => {
  assert.equal(trend(110, 100, TREND_DEADBAND), "up");
  assert.equal(trend(88, 100, TREND_DEADBAND), "down");
});

test("trend falls back to an absolute deadband at a zero baseline", () => {
  assert.equal(trend(0.01, 0, TREND_DEADBAND), "flat");
  assert.equal(trend(1, 0, TREND_DEADBAND), "up");
  assert.equal(trend(-1, 0, TREND_DEADBAND), "down");
});

test("sampleForTrend clamps to the oldest point on a young ring", () => {
  const history: SnapshotHistory = { cap: 10, points: [0, 1, 2, 3, 4].map(point) };
  const then = sampleForTrend(history, 150);
  assert.equal(then?.epoch_ms, 0);
});

test("sampleForTrend picks the exact offset when available", () => {
  const history: SnapshotHistory = {
    cap: 20,
    points: Array.from({ length: 20 }, (_, i) => point(i)),
  };
  const then = sampleForTrend(history, 5);
  assert.equal(then?.epoch_ms, 14);
});

test("sampleForTrend is null on an empty ring", () => {
  const history: SnapshotHistory = { cap: 10, points: [] };
  assert.equal(sampleForTrend(history, 150), null);
});

test("cardTrend is flat with no baseline", () => {
  assert.equal(cardTrend(50, null), "flat");
});

test("trendGlyph matches direction", () => {
  assert.equal(trendGlyph("up"), "↑");
  assert.equal(trendGlyph("down"), "↓");
  assert.equal(trendGlyph("flat"), "→");
});

test("trendTitle formats a signed delta with the given unit", () => {
  assert.equal(trendTitle(112, 100, "%"), "+12.0% vs 5 min ago");
  assert.equal(trendTitle(88, 100, "%"), "-12.0% vs 5 min ago");
  assert.equal(trendTitle(50, null, "%"), "no 5-minute baseline yet");
});

test("trendTone tints only the bad direction", () => {
  // Lock pressure / connections: up is bad.
  assert.equal(trendTone("up", true), "warn");
  assert.equal(trendTone("down", true), "");
  assert.equal(trendTone("flat", true), "");
  // Cache hit: down is bad.
  assert.equal(trendTone("down", false), "warn");
  assert.equal(trendTone("up", false), "");
});
