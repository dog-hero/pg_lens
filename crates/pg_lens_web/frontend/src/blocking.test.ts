// Unit tests for the blocking-chain walk — mirrors the Rust test suite in
// crates/pg_lens_core/src/blocking.rs so the two implementations stay in
// lockstep (same runner setup as waits.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { blockingChain } from "./blocking.ts";
import type { LockRow } from "./types.ts";

function lock(pid: number, blockedBy: number[]): LockRow {
  return {
    pid,
    blocked_by: blockedBy,
    mode: "ShareLock",
    locktype: "transactionid",
    relation: null,
    duration_secs: 1,
    query: "",
  };
}

test("a free pid has no chain", () => {
  const locks = [lock(2, [1])];
  assert.equal(blockingChain(99, locks), null);
});

test("direct pair chains to the single blocker", () => {
  const chain = blockingChain(2, [lock(2, [1])]);
  assert.deepEqual(chain, { chain: [2, 1], deadlock: false });
});

test("three-level chain walks to the root blocker", () => {
  // C waits on B, B waits on A, A is free (no LockRow of its own).
  const locks = [lock(3, [2]), lock(2, [1])];
  const chain = blockingChain(3, locks);
  assert.deepEqual(chain, { chain: [3, 2, 1], deadlock: false });
});

test("deadlock cycle is detected and bounded", () => {
  // A waits on B, B waits on A: a genuine wait-for cycle.
  const locks = [lock(1, [2]), lock(2, [1])];
  const chain = blockingChain(1, locks);
  assert.deepEqual(chain, { chain: [1, 2, 1], deadlock: true });
});

test("self-referencing row is a trivial deadlock", () => {
  const chain = blockingChain(1, [lock(1, [1])]);
  assert.deepEqual(chain, { chain: [1, 1], deadlock: true });
});
