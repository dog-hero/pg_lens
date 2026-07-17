// Unit tests for the idle-in-transaction / xact-age hunter — mirrors the
// Rust test suite in crates/pg_lens_core/src/xact_age.rs so the two
// implementations stay in lockstep (same runner setup as waits.test.ts).

import { test } from "node:test";
import assert from "node:assert/strict";

import { oldestOpenXact, xactAgeSeverity } from "./xact_age.ts";
import type { ActivityRow } from "./types.ts";

function row(state: string, xactAgeSecs: number | null, pid = 1): ActivityRow {
  return {
    pid,
    application_name: "",
    database: "",
    client: "",
    duration_secs: 0,
    xact_age_secs: xactAgeSecs,
    wait_event: null,
    username: "",
    state,
    query: "",
    query_leader_pid: pid,
    is_parallel_worker: false,
    query_id: null,
  };
}

test("severity tiers for a normal transaction", () => {
  assert.equal(xactAgeSeverity(0, "active"), "ok");
  assert.equal(xactAgeSeverity(300, "active"), "ok", "boundary is not yet warn");
  assert.equal(xactAgeSeverity(300.1, "active"), "warn");
  assert.equal(xactAgeSeverity(1_800, "active"), "warn", "boundary is not yet bad");
  assert.equal(xactAgeSeverity(1_800.1, "active"), "bad");
});

test("idle in transaction is worse than an equally old active query", () => {
  const age = 1_000;
  assert.equal(xactAgeSeverity(age, "active"), "warn");
  assert.equal(xactAgeSeverity(age, "idle in transaction"), "bad");
  assert.equal(xactAgeSeverity(age, "idle in transaction (aborted)"), "bad");
});

test("oldest open xact skips rows with no transaction", () => {
  const activity = [
    row("active", null, 1),
    row("active", 50, 2),
    row("idle in transaction", 2_000, 3),
    row("active", 1_500, 4),
  ];
  const oldest = oldestOpenXact(activity);
  assert.ok(oldest);
  assert.equal(oldest.row.pid, 3);
  assert.equal(oldest.severity, "bad");
});

test("oldest open xact is null when nothing has a transaction", () => {
  assert.equal(oldestOpenXact([row("idle", null, 1), row("active", null, 2)]), null);
});
