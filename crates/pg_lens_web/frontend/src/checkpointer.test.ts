// F4: checkpointer/bgwriter derivation — mirrors the TUI's
// ui/macro_lens.rs test suite so both implementations stay in lockstep
// (node:test, no framework, same setup as vacuum.test.ts).

import { test } from "node:test";
import assert from "node:assert/strict";

import type { CheckpointerStats } from "./types.ts";
import { checkpointPressureSeverity, checkpointerCard } from "./checkpointer.ts";

function stats(overrides: Partial<CheckpointerStats> = {}): CheckpointerStats {
  return {
    checkpoints_timed: 100,
    checkpoints_req: 10,
    checkpoint_write_time_ms: 50_000,
    checkpoint_sync_time_ms: 4_000,
    buffers_checkpoint: 900_000,
    buffers_clean: 30_000,
    maxwritten_clean: 5,
    buffers_backend: 20_000,
    buffers_alloc: 1_000_000,
    checkpoints_per_min_timed: 0.5,
    checkpoints_per_min_req: 0.02,
    buffers_checkpoint_per_sec: 12.3,
    buffers_clean_per_sec: 4.5,
    buffers_backend_per_sec: 1.1,
    avg_checkpoint_write_ms: 4_200,
    avg_checkpoint_sync_ms: 310,
    requested_ratio_session: 0.1,
    ...overrides,
  };
}

test("checkpoint pressure is calm below and at the fifty percent line", () => {
  assert.equal(checkpointPressureSeverity(null), "");
  assert.equal(checkpointPressureSeverity(0), "");
  assert.equal(checkpointPressureSeverity(0.5), "");
});

test("checkpoint pressure turns yellow once requested outweighs timed", () => {
  assert.equal(checkpointPressureSeverity(0.51), "warn");
  assert.equal(checkpointPressureSeverity(1), "warn");
});

test("card renders rates and a calm pressure line", () => {
  const card = checkpointerCard(stats({ requested_ratio_session: 0.1 }));
  assert.match(card.perMin, /0\.50 timed/);
  assert.match(card.perMin, /0\.02 req/);
  assert.match(card.pressure, /10% requested/);
  assert.match(card.buffersPerSec, /chkpt 12\.3\/s/);
  assert.match(card.buffersPerSec, /bgwriter 4\.5\/s/);
  assert.match(card.buffersPerSec, /backend 1\.1\/s/);
  assert.equal(card.severity, "");
});

test("card flags pressure and shows absent first-tick rates as --", () => {
  const under = checkpointerCard(stats({ requested_ratio_session: 0.9 }));
  assert.equal(under.severity, "warn");
  assert.match(under.pressure, /90% requested/);

  const first = checkpointerCard(
    stats({
      checkpoints_per_min_timed: null,
      checkpoints_per_min_req: null,
      buffers_checkpoint_per_sec: null,
      buffers_clean_per_sec: null,
      buffers_backend_per_sec: null,
      avg_checkpoint_write_ms: null,
      avg_checkpoint_sync_ms: null,
      requested_ratio_session: null,
    }),
  );
  assert.equal(first.perMin, "-- timed / -- req /min");
  assert.match(first.pressure, /no checkpoint yet this session/);
  assert.equal(first.avgWriteSync, "-- / --");
});

test("card explains the PG17 backend-buffers split instead of a bare dash", () => {
  const card = checkpointerCard(
    stats({ buffers_backend: null, buffers_backend_per_sec: null }),
  );
  assert.match(card.buffersPerSec, /backend n\/a \(17\+\)/);
});
