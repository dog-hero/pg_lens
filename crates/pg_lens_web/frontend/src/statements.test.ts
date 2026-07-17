// Unit tests for the Query Lens's filter matcher (v0.12) — mirrors the
// TUI's `statements_row_matches` in crates/pg_lens_tui/src/app.rs so both
// implementations stay in lockstep (same runner setup as waits.test.ts:
// node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { statementsRowMatches } from "./statements.ts";
import type { StatementRow } from "./types.ts";

/** Minimal row builder: only query/query_id matter to the matcher. */
function row(query: string, queryId: string | null = null): StatementRow {
  return {
    query_id: queryId,
    query,
    username: "app",
    calls: 0,
    total_exec_ms: 0,
    mean_exec_ms: 0,
    rows: 0,
    shared_blks_hit: 0,
    shared_blks_read: 0,
  };
}

test("matches the query text case-insensitively", () => {
  const r = row("UPDATE pgbench_accounts SET abalance = $1 WHERE aid = $2");
  assert.ok(statementsRowMatches(r, "pgbench_accounts"));
  assert.ok(statementsRowMatches(r, "update"));
  assert.ok(!statementsRowMatches(r, "delete"));
});

test("matches the queryid when present", () => {
  const r = row("SELECT 1", "3004918872215881003");
  assert.ok(statementsRowMatches(r, "3004918872215881003"));
  assert.ok(!statementsRowMatches(r, "9999999999999999999"));
});

test("a null queryid never matches, never throws", () => {
  const r = row("SELECT 1", null);
  assert.ok(!statementsRowMatches(r, "anything"));
});
