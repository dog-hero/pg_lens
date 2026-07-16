// Unit tests for the top-waits aggregation — mirrors the Rust test suite in
// crates/pg_lens_core/src/waits.rs so the two implementations stay in
// lockstep (same runner setup as sql.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { topWaits } from "./waits.ts";
import type { ActivityRow } from "./types.ts";

/** Minimal row builder: only wait_event matters to the fold. */
function row(wait: string | null): ActivityRow {
  return {
    pid: 1,
    application_name: "",
    database: "",
    client: "",
    duration_secs: 0,
    wait_event: wait,
    username: "",
    state: "active",
    query: "",
    query_leader_pid: 1,
    is_parallel_worker: false,
    query_id: null,
  };
}

test("ranks by count descending", () => {
  const summary = topWaits([
    row("IO:DataFileRead"),
    row("Lock:transactionid"),
    row("Lock:transactionid"),
    row("Lock:transactionid"),
    row("IO:DataFileRead"),
    row("Client:ClientRead"),
  ]);
  assert.deepEqual(summary.ranked, [
    ["Lock:transactionid", 3],
    ["IO:DataFileRead", 2],
    ["Client:ClientRead", 1],
  ]);
});

test("ties break alphabetically and deterministically", () => {
  const activity = [
    row("Lock:transactionid"),
    row("Client:ClientRead"),
    row("IO:DataFileRead"),
  ];
  const summary = topWaits(activity);
  assert.deepEqual(summary.ranked, [
    ["Client:ClientRead", 1],
    ["IO:DataFileRead", 1],
    ["Lock:transactionid", 1],
  ]);
  // Same input, same output — no map-insertion-order dependence.
  assert.deepEqual(topWaits(activity), summary);
});

test("running sessions are excluded but counted in total", () => {
  const summary = topWaits([
    row(null),
    row("Lock:relation"),
    row(null),
    row("Lock:relation"),
  ]);
  assert.equal(summary.waiting, 2);
  assert.equal(summary.total, 4);
  assert.deepEqual(summary.ranked, [["Lock:relation", 2]]);
});

test("empty and all-running yield an empty summary", () => {
  assert.deepEqual(topWaits([]), { waiting: 0, total: 0, ranked: [] });
  const summary = topWaits([row(null), row(null)]);
  assert.equal(summary.waiting, 0);
  assert.equal(summary.total, 2);
  assert.deepEqual(summary.ranked, []);
});
