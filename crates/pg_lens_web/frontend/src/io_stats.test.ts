// v0.16: I/O profile (pg_stat_io) card helpers — mirrors the TUI's
// ui/macro_lens.rs io_stats_lines test suite (node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import type { IoStatRow } from "./types.ts";
import { ioStatLine, ioStatLines } from "./io_stats.ts";

function row(overrides: Partial<IoStatRow> = {}): IoStatRow {
  return {
    backend_type: "client backend",
    context: "normal",
    reads: 1_000,
    writes: 200,
    writebacks: 0,
    extends: 0,
    hits: 9_000,
    evictions: 0,
    reuses: 0,
    fsyncs: 0,
    avg_read_ms: 0.18,
    avg_write_ms: 0.42,
    reads_per_sec: 1_380,
    writes_per_sec: 58,
    hit_ratio: 0.947,
    ...overrides,
  };
}

test("ioStatLine formats rate/timing/hit-ratio for one row", () => {
  const line = ioStatLine(row());
  assert.match(line, /^client backend\/normal: /);
  assert.match(line, /rd 1380\.0\/s \(0\.18ms\)/);
  assert.match(line, /wr 58\.0\/s \(0\.42ms\)/);
  assert.match(line, /hit 95%/);
});

test("ioStatLine shows dashes, never a fabricated zero, when timing is off", () => {
  const line = ioStatLine(
    row({ avg_read_ms: null, avg_write_ms: null, reads_per_sec: null, hit_ratio: null }),
  );
  assert.match(line, /rd -- \(--\)/);
  assert.match(line, /hit --/);
});

test("ioStatLines renders one calm line when the collection found nothing", () => {
  const lines = ioStatLines([]);
  assert.deepEqual(lines, ["no I/O activity recorded this interval"]);
});

test("ioStatLines caps at IO_ROWS_SHOWN", () => {
  const rows = Array.from({ length: 8 }, (_, i) => row({ backend_type: `backend${i}` }));
  const lines = ioStatLines(rows);
  assert.equal(lines.length, 4);
});
