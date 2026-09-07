import { test } from "node:test";
import assert from "node:assert/strict";

import {
  filterRecordRows,
  formatBytes,
  formatDuration,
} from "./records-lens.ts";
import type { RecordingEntry } from "./types.ts";

test("formatBytes formats various file sizes correctly", () => {
  assert.equal(formatBytes(500), "500 B");
  assert.equal(formatBytes(1024), "1.0 KB");
  assert.equal(formatBytes(1536), "1.5 KB");
  assert.equal(formatBytes(1048576), "1.0 MB");
  assert.equal(formatBytes(1073741824), "1.00 GB");
});

test("formatDuration formats seconds into readable strings", () => {
  assert.equal(formatDuration(null), "-");
  assert.equal(formatDuration(35), "35s");
  assert.equal(formatDuration(65), "1m 5s");
  assert.equal(formatDuration(3665), "1h 1m");
});

test("filterRecordRows filters by target, filename, or kind", () => {
  const records: RecordingEntry[] = [
    {
      path: "/path/to/rec-shop-20260907.jsonl",
      filename: "rec-shop-20260907.jsonl",
      target: "shop",
      kind: "Recording",
      size_bytes: 5000,
      started_at_secs: 1000,
      ended_at_secs: 1050,
      started_at: "2026-09-07 14:00:00",
      ended_at: "2026-09-07 14:50:00",
      duration_secs: 50,
      frame_count: 25,
      is_active: false,
    },
    {
      path: "/path/to/snapshot-analytics-20260907.json",
      filename: "snapshot-analytics-20260907.json",
      target: "analytics",
      kind: "Bookmark",
      size_bytes: 1200,
      started_at_secs: 1100,
      ended_at_secs: 1100,
      started_at: "2026-09-07 15:00:00",
      ended_at: "2026-09-07 15:00:00",
      duration_secs: 0,
      frame_count: 1,
      is_active: false,
    },
  ];

  assert.equal(filterRecordRows(records, "").length, 2);
  assert.equal(filterRecordRows(records, "shop").length, 1);
  assert.equal(filterRecordRows(records, "analytics").length, 1);
  assert.equal(filterRecordRows(records, "bookmark").length, 1);
  assert.equal(filterRecordRows(records, "nonexistent").length, 0);
});

