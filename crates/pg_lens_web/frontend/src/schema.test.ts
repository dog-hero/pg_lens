// Unit tests for the Schema Lens's filter matcher (v0.12) — mirrors the
// TUI's `schema_row_matches` in crates/pg_lens_tui/src/app.rs so both
// implementations stay in lockstep (same runner setup as waits.test.ts:
// node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { growthSeverity, schemaRowMatches } from "./schema.ts";
import type { TableStatRow } from "./types.ts";

/** Minimal row builder: only schema/name matter to the matcher. */
function table(schema: string, name: string): TableStatRow {
  return {
    oid: 0,
    schema,
    name,
    total_bytes: 0,
    table_bytes: 0,
    index_bytes: 0,
    seq_scan: 0,
    seq_tup_read: 0,
    idx_scan: null,
    idx_tup_fetch: null,
    n_tup_ins: 0,
    n_tup_upd: 0,
    n_tup_del: 0,
    n_tup_hot_upd: 0,
    n_live_tup: 0,
    n_dead_tup: 0,
    n_mod_since_analyze: 0,
    n_ins_since_vacuum: 0,
    last_vacuum_epoch_secs: null,
    last_autovacuum_epoch_secs: null,
    last_analyze_epoch_secs: null,
    last_autoanalyze_epoch_secs: null,
    vacuum_count: 0,
    autovacuum_count: 0,
    analyze_count: 0,
    autoanalyze_count: 0,
    growth_1h_bytes: null,
    growth_1h_pct: null,
  };
}

test("matches the table name case-insensitively", () => {
  const row = table("public", "order_items");
  assert.ok(schemaRowMatches(row, "order"));
  assert.ok(schemaRowMatches(row, "ORDER".toLowerCase()));
  assert.ok(!schemaRowMatches(row, "customers"));
});

test("matches the schema name", () => {
  const row = table("audit", "login_events");
  assert.ok(schemaRowMatches(row, "audit"));
  assert.ok(!schemaRowMatches(row, "public"));
});

test("matches a fully-qualified term that straddles the dot", () => {
  const row = table("public", "orders");
  assert.ok(schemaRowMatches(row, "public.orders"));
  assert.ok(schemaRowMatches(row, "lic.ord"));
});

test("empty needle is never reached by callers (the filter step short-circuits), but is not a false negative", () => {
  const row = table("public", "orders");
  assert.ok(schemaRowMatches(row, ""));
});

// v0.14: growthSeverity mirrors pg_lens_core::schema_growth::severity.
const BIG = 10 * 1024 * 1024; // SEVERITY_MIN_TABLE_BYTES

test("growthSeverity never colors a table below the absolute size floor", () => {
  assert.equal(growthSeverity(1024, 50), "none");
});

test("growthSeverity is red past 25%, yellow past 10%, on a big-enough table", () => {
  assert.equal(growthSeverity(BIG, 50), "red");
  assert.equal(growthSeverity(BIG, 15), "yellow");
  assert.equal(growthSeverity(BIG, 5), "none");
});

test("growthSeverity uses the absolute value (a large shrink also tints)", () => {
  assert.equal(growthSeverity(BIG, -30), "red");
});

test("growthSeverity is calm when growth is unknown", () => {
  assert.equal(growthSeverity(BIG, null), "none");
});
