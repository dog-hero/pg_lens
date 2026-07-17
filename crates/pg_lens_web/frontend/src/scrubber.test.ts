// Unit tests for the history scrubber's pure logic: timestamp→index
// resolution, aging-out, and readout formatting.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  cacheHitReadoutSeverity,
  formatPinAge,
  formatReadoutTime,
  lockPressureReadoutSeverity,
  readoutAtIndex,
  resolvePinnedIndex,
  toReadout,
} from "./scrubber.ts";
import type { HistoryPoint, SnapshotHistory } from "./types.ts";

function point(epochMs: number): HistoryPoint {
  return {
    epoch_ms: epochMs,
    tps: epochMs / 1000,
    active_sessions: epochMs % 100,
    connections_total: 10 + (epochMs % 10),
    cache_hit_pct: 95,
    lock_pressure_pct: 20,
    oldest_xid_age: 1000,
  };
}

function history(epochsMs: number[], cap = 100): SnapshotHistory {
  return { cap, points: epochsMs.map(point) };
}

test("toReadout maps a HistoryPoint 1:1", () => {
  const p = point(5000);
  const r = toReadout(p);
  assert.equal(r.epochMs, 5000);
  assert.equal(r.tps, 5);
  assert.equal(r.connectionsTotal, p.connections_total);
  assert.equal(r.cacheHitPct, 95);
  assert.equal(r.lockPressurePct, 20);
  assert.equal(r.oldestXidAge, 1000);
});

test("readoutAtIndex resolves an in-range index", () => {
  const h = history([1000, 2000, 3000]);
  const r = readoutAtIndex(h, 1);
  assert.equal(r?.epochMs, 2000);
});

test("readoutAtIndex is null for null/out-of-range indices", () => {
  const h = history([1000, 2000, 3000]);
  assert.equal(readoutAtIndex(h, null), null);
  assert.equal(readoutAtIndex(h, -1), null);
  assert.equal(readoutAtIndex(h, 3), null);
  assert.equal(readoutAtIndex({ cap: 10, points: [] }, 0), null);
});

test("resolvePinnedIndex finds an exact match", () => {
  const h = history([1000, 2000, 3000, 4000]);
  assert.equal(resolvePinnedIndex(h, 3000), 2);
});

test("resolvePinnedIndex finds the nearest neighbor", () => {
  const h = history([1000, 2000, 3000, 4000]);
  // 2600 is nearer to 3000 (400 away) than 2000 (600 away).
  assert.equal(resolvePinnedIndex(h, 2600), 2);
  // 2400 is nearer to 2000 (400 away) than 3000 (600 away).
  assert.equal(resolvePinnedIndex(h, 2400), 1);
  // exact tie prefers the earlier point.
  assert.equal(resolvePinnedIndex(h, 2500), 1);
});

test("resolvePinnedIndex clamps a timestamp past the newest point to the last index", () => {
  const h = history([1000, 2000, 3000]);
  assert.equal(resolvePinnedIndex(h, 9999), 2);
});

test("resolvePinnedIndex ages out a timestamp older than the ring's oldest point", () => {
  const h = history([5000, 6000, 7000]);
  assert.equal(resolvePinnedIndex(h, 1000), null);
});

test("resolvePinnedIndex is null on an empty ring", () => {
  assert.equal(resolvePinnedIndex({ cap: 10, points: [] }, 1000), null);
});

test("resolvePinnedIndex survives a ring eviction (pin index would have shifted)", () => {
  // Simulates the ring after the oldest points were evicted by new pushes —
  // the pinned timestamp (3000) is still present, just at a new index.
  const before = history([1000, 2000, 3000, 4000]);
  const pinnedIdx = resolvePinnedIndex(before, 3000);
  assert.equal(pinnedIdx, 2);

  const after = history([2000, 3000, 4000, 5000, 6000]);
  const stillPinned = resolvePinnedIndex(after, 3000);
  assert.equal(stillPinned, 1);
  assert.equal(after.points[stillPinned ?? -1]?.epoch_ms, 3000);
});

test("cacheHitReadoutSeverity warns under 90%, null is calm", () => {
  assert.equal(cacheHitReadoutSeverity(null), "");
  assert.equal(cacheHitReadoutSeverity(95), "");
  assert.equal(cacheHitReadoutSeverity(89.9), "warn");
});

test("lockPressureReadoutSeverity mirrors lock_capacity.ts's thresholds", () => {
  assert.equal(lockPressureReadoutSeverity(null), "");
  assert.equal(lockPressureReadoutSeverity(50), "");
  assert.equal(lockPressureReadoutSeverity(70), "warn");
  assert.equal(lockPressureReadoutSeverity(90), "bad");
});

test("formatReadoutTime renders a HH:MM:SS local time string", () => {
  const t = formatReadoutTime(Date.UTC(2026, 0, 1, 12, 0, 0));
  assert.match(t, /^\d{1,2}:\d{2}:\d{2}\s?(AM|PM)?$/);
});

test("formatPinAge reads 'just now' under a minute, then rounds to minutes", () => {
  const now = 1_000_000;
  assert.equal(formatPinAge(now - 5_000, now), "just now");
  assert.equal(formatPinAge(now - 65_000, now), "1m ago");
  assert.equal(formatPinAge(now - 300_000, now), "5m ago");
});
