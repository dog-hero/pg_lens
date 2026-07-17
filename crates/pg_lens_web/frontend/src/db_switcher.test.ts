// Unit tests for the header database switcher's pure helpers (v0.13) —
// same node:test/no-framework setup as idle_sessions.test.ts.

import { test } from "node:test";
import assert from "node:assert/strict";

import { dbOptionLabel, hasSwitchableDatabases } from "./db_switcher.ts";
import type { DatabaseRow } from "./types.ts";

test("dbOptionLabel shows a human-readable size", () => {
  const row: DatabaseRow = { name: "shop", size_bytes: 3_400_000_000 };
  assert.equal(dbOptionLabel(row), "shop (3.2 GB)");
});

test("dbOptionLabel shows a placeholder when the size is unreadable", () => {
  const row: DatabaseRow = { name: "analytics", size_bytes: null };
  assert.equal(dbOptionLabel(row), "analytics (?)");
});

test("hasSwitchableDatabases is false when databases is null (restricted role)", () => {
  assert.equal(hasSwitchableDatabases(null), false);
});

test("hasSwitchableDatabases is false with a single database (nothing to switch to)", () => {
  assert.equal(hasSwitchableDatabases([{ name: "shop", size_bytes: 100 }]), false);
});

test("hasSwitchableDatabases is true with two or more databases", () => {
  assert.equal(
    hasSwitchableDatabases([
      { name: "shop", size_bytes: 100 },
      { name: "warehouse", size_bytes: 200 },
    ]),
    true,
  );
});
