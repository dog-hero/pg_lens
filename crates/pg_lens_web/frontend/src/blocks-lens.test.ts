import { test } from "node:test";
import assert from "node:assert/strict";

import { blockNodeBadge, countTreeNodes, lockStatusBadge } from "./blocks-lens.ts";
import type { BlockTreeNode } from "./types.ts";

function createNode(pid: number, isRoot: boolean, isDeadlock: boolean, descendants: number): BlockTreeNode {
  return {
    pid,
    usename: "postgres",
    application_name: "psql",
    state: "active",
    query: "SELECT 1;",
    duration_secs: 5,
    wait_event: "Lock:relation",
    mode: "ExclusiveLock",
    relation: "orders",
    is_root: isRoot,
    num_descendants: descendants,
    is_deadlock: isDeadlock,
    children: [],
  };
}

test("lockStatusBadge returns correct badge for granted and waiting locks", () => {
  assert.deepEqual(lockStatusBadge(true), { text: "GRANT", className: "badge badge-ok" });
  assert.deepEqual(lockStatusBadge(false), { text: "WAIT", className: "badge badge-bad" });
});

test("blockNodeBadge labels deadlock over root blocker", () => {
  const node = createNode(100, true, true, 2);
  const badge = blockNodeBadge(node);
  assert.deepEqual(badge, { text: "DEADLOCK", className: "badge badge-bad" });
});

test("blockNodeBadge formats root blocker with single waiter", () => {
  const node = createNode(101, true, false, 1);
  const badge = blockNodeBadge(node);
  assert.deepEqual(badge, { text: "ROOT BLOCKER (1 waiter)", className: "badge badge-bad" });
});

test("blockNodeBadge formats root blocker with multiple waiters", () => {
  const node = createNode(102, true, false, 3);
  const badge = blockNodeBadge(node);
  assert.deepEqual(badge, { text: "ROOT BLOCKER (3 waiters)", className: "badge badge-bad" });
});

test("blockNodeBadge returns null for normal blocked node", () => {
  const node = createNode(103, false, false, 0);
  assert.equal(blockNodeBadge(node), null);
});

test("countTreeNodes counts nested tree nodes accurately", () => {
  const root = createNode(1, true, false, 2);
  const child1 = createNode(2, false, false, 1);
  const child2 = createNode(3, false, false, 0);
  child1.children = [child2];
  root.children = [child1];

  assert.equal(countTreeNodes([root]), 3);
});
