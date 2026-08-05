// Unit tests for the Schema Lens's filter matcher (v0.12) — mirrors the
// TUI's `schema_row_matches` in crates/pg_lens_tui/src/app.rs so both
// implementations stay in lockstep (same runner setup as waits.test.ts:
// node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  columnDefaultText,
  columnDetailLines,
  growthSeverity,
  indexDetailLines,
  ownConstraintLines,
  partitionsHint,
  referencingConstraintLines,
  schemaRowMatches,
  structureErrorLine,
  tableCountText,
} from "./schema.ts";
import type { TableDetail, TableStatRow } from "./types.ts";

/** Minimal row builder: only schema/name matter to the matcher. Every
 * v0.15 partition field defaults to "plain, non-partitioned table" —
 * callers that need a parent/leaf shape pass `overrides`. */
function table(
  schema: string,
  name: string,
  overrides: Partial<TableStatRow> = {},
): TableStatRow {
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
    is_partition: false,
    parent_oid: null,
    partition_count: null,
    lock_count: null,
    lock_waiters: null,
    ...overrides,
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

// v0.15: honest table counts — mirrors the TUI's `table_count_text`.
test("tableCountText is plain when the fetched list is complete and unfiltered", () => {
  assert.equal(tableCountText(4, 4, 4), "4 tables");
  // Total not known yet (before the first successful collection).
  assert.equal(tableCountText(4, 4, null), "4 tables");
});

test("tableCountText flags truncation when the total exceeds the fetched list", () => {
  assert.equal(
    tableCountText(200, 200, 250),
    "200 of 250 tables — raise schema_table_limit or filter",
  );
});

test("tableCountText composes with an active filter (shown/fetched)", () => {
  assert.equal(tableCountText(3, 200, 200), "3/200 tables");
});

test("tableCountText composes a filter AND truncation together", () => {
  assert.equal(
    tableCountText(3, 200, 250),
    "3/200 of 250 tables — raise schema_table_limit or filter",
  );
});

// v0.15: on-demand `\d`-style table detail render helpers.

function detail(overrides: Partial<TableDetail> = {}): TableDetail {
  return {
    oid: 16_405,
    schema: "public",
    name: "order_items",
    collected_at_epoch_ms: 0,
    columns: [],
    constraints: [],
    indexes: [],
    error: null,
    ...overrides,
  };
}

test("structureErrorLine renders the best-effort failure reason inline", () => {
  assert.equal(
    structureErrorLine("relation does not exist"),
    "structure unavailable: relation does not exist",
  );
});

test("columnDetailLines formats name, type, nullability, and default", () => {
  const d = detail({
    columns: [
      {
        name: "id",
        data_type: "bigint",
        not_null: true,
        default: "nextval('s')",
        identity: null,
        generated_stored: false,
      },
      {
        name: "notes",
        data_type: "text",
        not_null: false,
        default: null,
        identity: null,
        generated_stored: false,
      },
    ],
  });
  assert.deepEqual(columnDetailLines(d), [
    "id bigint not null default nextval('s')",
    "notes text null",
  ]);
});

// v0.15: identity/stored-generated columns.

test("columnDefaultText renders a plain default", () => {
  assert.equal(
    columnDefaultText({
      name: "qty",
      data_type: "integer",
      not_null: true,
      default: "1",
      identity: null,
      generated_stored: false,
    }),
    " default 1",
  );
});

test("columnDefaultText renders no clause when there is neither a default nor identity/generation", () => {
  assert.equal(
    columnDefaultText({
      name: "notes",
      data_type: "text",
      not_null: false,
      default: null,
      identity: null,
      generated_stored: false,
    }),
    "",
  );
});

test("columnDefaultText renders identity columns via the friendly label, never a default", () => {
  assert.equal(
    columnDefaultText({
      name: "id",
      data_type: "bigint",
      not_null: true,
      default: null,
      identity: "generated always as identity",
      generated_stored: false,
    }),
    " generated always as identity",
  );
  assert.equal(
    columnDefaultText({
      name: "id",
      data_type: "bigint",
      not_null: true,
      default: null,
      identity: "generated by default as identity",
      generated_stored: false,
    }),
    " generated by default as identity",
  );
});

test("columnDefaultText prefixes a STORED generated column's expression", () => {
  assert.equal(
    columnDefaultText({
      name: "total_price",
      data_type: "numeric(14,2)",
      not_null: false,
      default: "(qty * price)",
      identity: null,
      generated_stored: true,
    }),
    " generated always as ((qty * price)) stored",
  );
});

test("columnDetailLines renders the identity clause for an identity column", () => {
  const d = detail({
    columns: [
      {
        name: "id",
        data_type: "bigint",
        not_null: true,
        default: null,
        identity: "generated always as identity",
        generated_stored: false,
      },
    ],
  });
  assert.deepEqual(columnDetailLines(d), [
    "id bigint not null generated always as identity",
  ]);
});

test("ownConstraintLines excludes referencing rows, referencingConstraintLines includes only them", () => {
  const d = detail({
    constraints: [
      {
        name: "order_items_pkey",
        kind: "PRIMARY KEY",
        definition: "PRIMARY KEY (id)",
        referencing_table: null,
      },
      {
        name: "order_item_notes_item_id_fkey",
        kind: "FOREIGN KEY",
        definition: "FOREIGN KEY (item_id) REFERENCES order_items(id)",
        referencing_table: "order_item_notes",
      },
    ],
  });
  assert.deepEqual(ownConstraintLines(d), [
    "PRIMARY KEY order_items_pkey: PRIMARY KEY (id)",
  ]);
  assert.deepEqual(referencingConstraintLines(d), [
    "order_item_notes.order_item_notes_item_id_fkey: FOREIGN KEY (item_id) REFERENCES order_items(id)",
  ]);
});

test("indexDetailLines is the verbatim pg_get_indexdef output, one per index", () => {
  const d = detail({
    indexes: [
      { name: "order_items_pkey", definition: "CREATE UNIQUE INDEX ... USING btree (id)" },
    ],
  });
  assert.deepEqual(indexDetailLines(d), ["CREATE UNIQUE INDEX ... USING btree (id)"]);
});

// v0.15's partition collapsing.

test("partitionsHint is empty when the database has no partitioned tables", () => {
  const rows = [table("public", "orders"), table("public", "customers")];
  assert.equal(partitionsHint(rows, false), "");
  assert.equal(partitionsHint(rows, true), "");
});

test("partitionsHint counts hidden leaves when collapsed, and flips when expanded", () => {
  const rows = [
    table("public", "events", { partition_count: 2 }),
    table("public", "events_2026_06", { is_partition: true, parent_oid: 0 }),
    table("public", "events_2026_07", { is_partition: true, parent_oid: 0 }),
  ];
  assert.equal(partitionsHint(rows, false), " · +2 parts");
  assert.equal(partitionsHint(rows, true), " · parts shown");
});
