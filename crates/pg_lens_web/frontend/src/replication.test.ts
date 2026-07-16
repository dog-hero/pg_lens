// Unit tests for the replication slot severity rule (F2.5) — mirrors the
// TUI's ui/macro_lens.rs `slot_severity` test suite so both implementations
// stay in lockstep (same runner setup as vacuum.test.ts: node:test, no
// framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { slotSeverity } from "./replication.ts";
import type { ReplicationSlotRow } from "./types.ts";

function slot(
  active: boolean,
  wal_status: string | null,
  retained_wal_bytes: number | null,
): ReplicationSlotRow {
  return {
    slot_name: "probe_slot",
    slot_type: "physical",
    active,
    retained_wal_bytes,
    wal_status,
    safe_wal_size: null,
  };
}

test("active reserved slot is calm, even retaining a lot", () => {
  assert.equal(slotSeverity(slot(true, "reserved", 0)), "");
  assert.equal(slotSeverity(slot(true, "reserved", 20 * 1024 * 1024 * 1024)), "");
});

test("inactive slot retaining WAL is yellow then red", () => {
  assert.equal(slotSeverity(slot(false, "extended", 0)), "", "retaining nothing stays calm");
  assert.equal(slotSeverity(slot(false, "extended", 1024)), "warn");
  assert.equal(slotSeverity(slot(false, "extended", 11 * 1024 * 1024 * 1024)), "bad");
});

test("unreserved or lost wal_status is always red", () => {
  assert.equal(slotSeverity(slot(false, "unreserved", 1024)), "bad");
  assert.equal(slotSeverity(slot(false, "lost", null)), "bad");
  assert.equal(slotSeverity(slot(true, "unreserved", 0)), "bad");
});
