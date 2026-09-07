import { test } from "node:test";
import assert from "node:assert/strict";

import { slruCard } from "./vitals.ts";
import type { SlruStats } from "./types.ts";

test("slruCard returns null when stats are null or rows empty", () => {
  assert.equal(slruCard(null), null);
  assert.equal(
    slruCard({
      collected_at_epoch_ms: Date.now(),
      rows: [],
      overall_hit_ratio_pct: null,
      subtrans_warning: false,
    }),
    null,
  );
});

test("slruCard formats hit ratio, subsystem summary, and tone", () => {
  const slru: SlruStats = {
    collected_at_epoch_ms: Date.now(),
    rows: [
      {
        name: "clog",
        blks_zeroed: 0,
        blks_hit: 1000,
        blks_read: 10,
        blks_written: 5,
        blks_exists: 0,
        flushes: 0,
        truncates: 0,
        hit_ratio_pct: 99.0,
        reads_per_sec: 1.0,
        writes_per_sec: 0.5,
        flushes_per_sec: 0.0,
      },
      {
        name: "subtrans",
        blks_zeroed: 0,
        blks_hit: 500,
        blks_read: 5,
        blks_written: 1,
        blks_exists: 0,
        flushes: 0,
        truncates: 0,
        hit_ratio_pct: 99.0,
        reads_per_sec: 0.5,
        writes_per_sec: 0.1,
        flushes_per_sec: 0.0,
      },
    ],
    overall_hit_ratio_pct: 99.0,
    subtrans_warning: false,
  };

  const card = slruCard(slru);
  assert.ok(card !== null);
  assert.equal(card?.label, "SLRU caches");
  assert.equal(card?.value, "99.0%");
  assert.equal(card?.detail, "clog 99% · subtrans 99%");
  assert.equal(card?.tone, "");
  assert.equal(card?.meter, 0.99);
});

test("slruCard marks bad tone and subtrans warning when thrashing occurs", () => {
  const thrashingSlru: SlruStats = {
    collected_at_epoch_ms: Date.now(),
    rows: [
      {
        name: "subtrans",
        blks_zeroed: 0,
        blks_hit: 100,
        blks_read: 500,
        blks_written: 10,
        blks_exists: 0,
        flushes: 0,
        truncates: 0,
        hit_ratio_pct: 16.7,
        reads_per_sec: 50.0,
        writes_per_sec: 1.0,
        flushes_per_sec: 0.0,
      },
    ],
    overall_hit_ratio_pct: 16.7,
    subtrans_warning: true,
  };

  const card = slruCard(thrashingSlru);
  assert.ok(card !== null);
  assert.equal(card?.tone, "bad");
  assert.match(card?.detail ?? "", /! SUBTRANS THRASHING/);
});

