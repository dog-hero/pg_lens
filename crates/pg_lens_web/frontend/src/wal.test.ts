// v0.16: WAL generation rate — mirrors the TUI's
// `ui/replication.rs` wal_* test suite so both implementations stay in
// lockstep (node:test, no framework, same setup as checkpointer.test.ts).

import { test } from "node:test";
import assert from "node:assert/strict";

import type { WalStats } from "./types.ts";
import { walBuffersFullSeverity, walGenerationText } from "./wal.ts";

function stats(overrides: Partial<WalStats> = {}): WalStats {
  return {
    wal_records: 1_000_000,
    wal_fpi: 10_000,
    wal_bytes: 500_000_000,
    wal_buffers_full: 0,
    wal_write_time_ms: 1_000,
    wal_sync_time_ms: 100,
    wal_bytes_per_sec: 2_000_000,
    wal_records_per_sec: 400,
    wal_buffers_full_delta: 0,
    ...overrides,
  };
}

test("wal generation is calm when buffers_full is not climbing", () => {
  assert.equal(walBuffersFullSeverity(stats({ wal_buffers_full_delta: 0 })), "");
  // Nonzero from history, but not climbing THIS tick — still calm.
  assert.equal(
    walBuffersFullSeverity(stats({ wal_buffers_full: 40, wal_buffers_full_delta: 0 })),
    "",
  );
  // No delta window yet (first poll) — calm, not a fault.
  assert.equal(
    walBuffersFullSeverity(
      stats({ wal_bytes_per_sec: null, wal_records_per_sec: null, wal_buffers_full_delta: null }),
    ),
    "",
  );
});

test("wal generation warns when buffers_full is actively climbing", () => {
  assert.equal(
    walBuffersFullSeverity(stats({ wal_buffers_full: 15, wal_buffers_full_delta: 3 })),
    "warn",
  );
});

test("wal generation text dashes rates before the first delta window", () => {
  const text = walGenerationText(
    stats({ wal_bytes_per_sec: null, wal_records_per_sec: null, wal_buffers_full_delta: null }),
  );
  assert.match(text, /--/);
  assert.match(text, /buffers_full: 0/);
});

test("wal generation text shows the climbing delta", () => {
  const text = walGenerationText(
    stats({ wal_buffers_full: 15, wal_buffers_full_delta: 3 }),
  );
  assert.match(text, /buffers_full: 15 \(\+3 this tick\)/);
});
