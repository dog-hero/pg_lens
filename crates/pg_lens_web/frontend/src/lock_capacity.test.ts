// Unit tests for the lock-table pressure severity thresholds — mirrors the
// Rust core's lock_capacity.rs test suite so both implementations stay in
// lockstep (same runner setup as prepared_xacts.test.ts: node:test, no
// framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { lockCapacitySeverity } from "./lock_capacity.ts";

test("severity tiers match the thresholds", () => {
  assert.equal(lockCapacitySeverity(0), "");
  assert.equal(lockCapacitySeverity(0.6), "", "boundary is not yet warn");
  assert.equal(lockCapacitySeverity(0.6001), "warn");
  assert.equal(lockCapacitySeverity(0.85), "warn", "boundary is not yet bad");
  assert.equal(lockCapacitySeverity(0.8501), "bad");
  assert.equal(lockCapacitySeverity(1), "bad");
});
