// Unit tests for the orphaned-2PC severity thresholds — mirrors the Rust
// core's prepared_xacts.rs test suite so both implementations stay in
// lockstep (same runner setup as vacuum.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { preparedXactSeverity } from "./prepared_xacts.ts";

test("severity tiers match the thresholds", () => {
  assert.equal(preparedXactSeverity(0), "");
  assert.equal(preparedXactSeverity(300), "", "boundary is not yet warn");
  assert.equal(preparedXactSeverity(300.1), "warn");
  assert.equal(preparedXactSeverity(3_600), "warn", "boundary is not yet bad");
  assert.equal(preparedXactSeverity(3_600.1), "bad");
  assert.equal(preparedXactSeverity(86_400), "bad");
});
