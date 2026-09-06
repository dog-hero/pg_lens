import { test } from "node:test";
import assert from "node:assert/strict";

import { filterProgressRows, formatProgressGauge, unifyProgress } from "./progress-lens.ts";
import type { DdlProgressRow, VacuumProgressRow } from "./types.ts";

test("unifyProgress returns empty array when inputs are null or empty", () => {
  assert.deepEqual(unifyProgress(null, null), []);
  assert.deepEqual(unifyProgress([], []), []);
});

test("unifyProgress unifies ddl_progress and vacuum_progress sorted by pid", () => {
  const ddl: DdlProgressRow[] = [
    {
      pid: 200,
      command: "CREATE INDEX CONCURRENTLY",
      relation: "public.orders_idx",
      phase: "building index: scan table",
      progress_pct: 45.5,
      current_step: 455,
      total_step: 1000,
      detail: "scanning",
    },
  ];

  const vac: VacuumProgressRow[] = [
    {
      pid: 100,
      relation: "public.users",
      phase: "scanning heap",
      heap_blks_total: 500,
      heap_blks_scanned: 250,
    },
  ];

  const unified = unifyProgress(ddl, vac);
  assert.equal(unified.length, 2);

  const u0 = unified[0];
  const u1 = unified[1];
  assert.ok(u0);
  assert.ok(u1);

  // Sorted by PID
  assert.equal(u0.pid, 100);
  assert.equal(u0.command, "VACUUM");
  assert.equal(u0.relation, "public.users");
  assert.equal(u0.phase, "scanning heap");
  assert.equal(u0.progress_pct, 50.0);
  assert.equal(u0.unit, "blocks");
  assert.equal(u0.detail, "heap blks: 250 / 500");

  assert.equal(u1.pid, 200);
  assert.equal(u1.command, "CREATE INDEX CONCURRENTLY");
  assert.equal(u1.relation, "public.orders_idx");
  assert.equal(u1.progress_pct, 45.5);
  assert.equal(u1.unit, "steps");
});

test("unifyProgress handles zero total blocks in vacuum", () => {
  const vac: VacuumProgressRow[] = [
    {
      pid: 105,
      relation: "public.empty_tbl",
      phase: "initializing",
      heap_blks_total: 0,
      heap_blks_scanned: 0,
    },
  ];

  const unified = unifyProgress(null, vac);
  assert.equal(unified.length, 1);
  const u0 = unified[0];
  assert.ok(u0);
  assert.equal(u0.progress_pct, null);
  assert.equal(u0.detail, "");
});

test("filterProgressRows filters by pid, command, relation, or phase", () => {
  const rows = unifyProgress(
    [
      {
        pid: 200,
        command: "CREATE INDEX",
        relation: "public.orders",
        phase: "waiting",
        progress_pct: null,
        current_step: 1,
        total_step: 3,
        detail: "lock wait",
      },
    ],
    [
      {
        pid: 100,
        relation: "public.customers",
        phase: "vacuuming indexes",
        heap_blks_total: 1000,
        heap_blks_scanned: 100,
      },
    ],
  );

  assert.equal(filterProgressRows(rows, "").length, 2);
  assert.equal(filterProgressRows(rows, "orders").length, 1);
  assert.equal(filterProgressRows(rows, "100").length, 1);
  assert.equal(filterProgressRows(rows, "vacuum").length, 1);
  assert.equal(filterProgressRows(rows, "INDEX").length, 2); // both have index in cmd or phase
  assert.equal(filterProgressRows(rows, "nonexistent").length, 0);
});

test("formatProgressGauge formats percentages and nulls cleanly", () => {
  assert.equal(formatProgressGauge(null), "in progress");
  assert.equal(formatProgressGauge(42.678), "42.7%");
  assert.equal(formatProgressGauge(100), "100.0%");
  assert.equal(formatProgressGauge(0), "0.0%");
});
