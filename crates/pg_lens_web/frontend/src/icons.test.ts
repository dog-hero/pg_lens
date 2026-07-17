import { test } from "node:test";
import assert from "node:assert/strict";

import { navIconId, severityIconId } from "./icons.ts";

test("navIconId maps every nav section to a distinct symbol id", () => {
  const sections = ["activity", "replication", "schema", "indexes", "queries"] as const;
  const ids = sections.map(navIconId);
  assert.equal(new Set(ids).size, ids.length, "ids must be distinct");
  for (const id of ids) {
    assert.match(id, /^icon-/);
  }
});

test("severityIconId returns a symbol id for every severity tier", () => {
  for (const s of ["ok", "warn", "bad", "info"] as const) {
    assert.match(severityIconId(s), /^icon-/);
  }
});
