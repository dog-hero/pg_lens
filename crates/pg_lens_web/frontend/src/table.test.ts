// Unit tests for the Micro Lens activity table's pure row-color decision
// (v0.16, Part A) — mirrors the TUI's `row_severity_style`/`state_row_color`
// tests in crates/pg_lens_tui/src/ui/micro_lens.rs so both implementations
// stay in lockstep (same runner setup as statements.test.ts: node:test, no
// framework, no DOM).

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  ROW_DURATION_BAD_SECS,
  ROW_DURATION_WARN_SECS,
  activityRowClass,
} from "./table.ts";
import type { ActivityRow } from "./types.ts";

function row(state: string, duration_secs: number): ActivityRow {
  return {
    pid: 1,
    application_name: "app",
    database: "db",
    client: "10.0.0.1",
    duration_secs,
    xact_age_secs: null,
    wait_event: null,
    username: "u",
    state,
    query: "SELECT 1",
    query_leader_pid: 1,
    is_parallel_worker: false,
    query_id: null,
  };
}

test("state base color maps every known state, neutral default for unknown", () => {
  assert.equal(activityRowClass(row("active", 0), false, false), "row-state-active");
  assert.equal(activityRowClass(row("idle", 0), false, false), "row-state-idle");
  assert.equal(
    activityRowClass(row("idle in transaction", 0), false, false),
    "row-state-idle-txn",
  );
  assert.equal(
    activityRowClass(row("idle in transaction (aborted)", 0), false, false),
    "row-state-idle-txn-aborted",
  );
  assert.equal(activityRowClass(row("fastpath function call", 0), false, false), "");
  assert.equal(activityRowClass(row("disabled", 0), false, false), "");
});

test("duration override only fires for active sessions, past the exact thresholds", () => {
  assert.equal(
    activityRowClass(row("active", ROW_DURATION_BAD_SECS + 0.1), false, false),
    "row-duration-bad",
  );
  assert.equal(
    activityRowClass(row("active", ROW_DURATION_WARN_SECS + 0.1), false, false),
    "row-duration-warn",
  );
  // At/below warn: no override, plain active color.
  assert.equal(
    activityRowClass(row("active", ROW_DURATION_WARN_SECS), false, false),
    "row-state-active",
  );
  // Same duration on an idle(-in-transaction) session must NOT turn
  // red/yellow — the owner's explicit "idle stays its state color
  // regardless of age" requirement.
  assert.equal(
    activityRowClass(row("idle", ROW_DURATION_BAD_SECS + 10_000), false, false),
    "row-state-idle",
  );
  assert.equal(
    activityRowClass(row("idle in transaction", ROW_DURATION_BAD_SECS + 10_000), false, false),
    "row-state-idle-txn",
  );
});

test("blocked wins over the duration override and over waiting", () => {
  const r = row("active", ROW_DURATION_BAD_SECS + 1);
  assert.equal(activityRowClass(r, true, false), "blocked");
  assert.equal(activityRowClass(r, true, true), "blocked");
});

test("waiting tints when nothing stronger applies", () => {
  assert.equal(activityRowClass(row("active", 1), false, true), "waiting");
});
