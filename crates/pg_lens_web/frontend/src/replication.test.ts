// Unit tests for the replication slot severity rule (F2.5) — mirrors the
// TUI's ui/macro_lens.rs `slot_severity` test suite so both implementations
// stay in lockstep (same runner setup as vacuum.test.ts: node:test, no
// framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { conflictsSeverity, conflictsText, slotSeverity } from "./replication.ts";
import type { DatabaseConflicts, ReplicationSlotRow } from "./types.ts";

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

test("conflictsSeverity: red if rate > 0, yellow if total > 0, calm otherwise", () => {
  const zeroConflicts: DatabaseConflicts = {
    datid: 12345,
    datname: "testdb",
    confl_tablespace: 0,
    confl_lock: 0,
    confl_snapshot: 0,
    confl_bufferpin: 0,
    confl_deadlock: 0,
    confl_total: 0,
    conflicts_per_sec: 0,
    lock_conflicts_per_sec: 0,
    snapshot_conflicts_per_sec: 0,
    deadlock_conflicts_per_sec: 0,
  };
  assert.equal(conflictsSeverity(zeroConflicts), "");

  const historicConflicts: DatabaseConflicts = {
    ...zeroConflicts,
    confl_total: 10,
    confl_lock: 10,
    conflicts_per_sec: 0,
  };
  assert.equal(conflictsSeverity(historicConflicts), "warn");

  const activeConflicts: DatabaseConflicts = {
    ...zeroConflicts,
    confl_total: 15,
    confl_lock: 15,
    conflicts_per_sec: 2.5,
  };
  assert.equal(conflictsSeverity(activeConflicts), "bad");
});

test("conflictsText formats breakdown and delta rates", () => {
  const c: DatabaseConflicts = {
    datid: 12345,
    datname: "testdb",
    confl_tablespace: 1,
    confl_lock: 5,
    confl_snapshot: 3,
    confl_bufferpin: 2,
    confl_deadlock: 0,
    confl_total: 11,
    conflicts_per_sec: 1.2,
    lock_conflicts_per_sec: 0.5,
    snapshot_conflicts_per_sec: 0.3,
    deadlock_conflicts_per_sec: 0,
  };
  assert.equal(
    conflictsText(c),
    "conflicts: 11 (1.2/s) · lock 5 · snapshot 3 · deadlock 0 · pin 2 · tblspc 1",
  );
});
