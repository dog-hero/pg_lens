// Unit tests for the vacuum/XID-wraparound severity thresholds — mirrors
// the TUI's ui/vacuum.rs test suite so both implementations stay in
// lockstep (same runner setup as waits.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { ageSeverity } from "./vacuum.ts";

test("severity tiers match the spec thresholds", () => {
  assert.equal(ageSeverity(0), "");
  assert.equal(ageSeverity(199_999_999), "");
  assert.equal(ageSeverity(200_000_000), "", "boundary is not yet warn");
  assert.equal(ageSeverity(200_000_001), "warn");
  assert.equal(ageSeverity(499_999_999), "warn");
  assert.equal(ageSeverity(500_000_000), "warn", "boundary is not yet bad");
  assert.equal(ageSeverity(500_000_001), "bad");
  assert.equal(ageSeverity(2_100_000_000), "bad");
});
