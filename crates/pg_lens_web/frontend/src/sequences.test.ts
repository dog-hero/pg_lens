import { test } from "node:test";
import assert from "node:assert/strict";

import {
  formatCurrent,
  formatPercentUsed,
  formatRemaining,
  sequenceMarker,
  sequenceSeverityClass,
  sequencesSummaryText,
} from "./sequences.ts";
import type { SequenceRow } from "./types.ts";

test("sequenceMarker maps severity tiers correctly", () => {
  assert.equal(sequenceMarker("Critical"), "!!");
  assert.equal(sequenceMarker("Warning"), "!");
  assert.equal(sequenceMarker("Normal"), "");
});

test("sequenceSeverityClass maps severity tiers correctly", () => {
  assert.equal(sequenceSeverityClass("Critical"), "seq-crit");
  assert.equal(sequenceSeverityClass("Warning"), "seq-warn");
  assert.equal(sequenceSeverityClass("Normal"), "");
});

test("formatPercentUsed formats percentage or dash", () => {
  assert.equal(formatPercentUsed(84.23), "84.2%");
  assert.equal(formatPercentUsed(0.0), "0.0%");
  assert.equal(formatPercentUsed(null), "—");
});

test("formatRemaining and formatCurrent format numbers or dash", () => {
  assert.equal(formatRemaining(1500000), "1,500,000");
  assert.equal(formatRemaining(null), "—");
  assert.equal(formatCurrent(42), "42");
  assert.equal(formatCurrent(null), "—");
});

test("sequencesSummaryText summarizes counts and warnings", () => {
  const seqs: SequenceRow[] = [
    {
      schema: "public",
      sequence_name: "orders_id_seq",
      data_type: "integer",
      start_value: 1,
      min_value: 1,
      max_value: 2147483647,
      increment_by: 1,
      cycle: false,
      last_value: 2000000000,
      table_name: "orders",
      column_name: "id",
      percent_used: 93.1,
      remaining_count: 147483647,
      severity: "Critical",
    },
    {
      schema: "public",
      sequence_name: "items_id_seq",
      data_type: "smallint",
      start_value: 1,
      min_value: 1,
      max_value: 32767,
      increment_by: 1,
      cycle: false,
      last_value: 26000,
      table_name: "items",
      column_name: "id",
      percent_used: 79.3,
      remaining_count: 6767,
      severity: "Warning",
    },
    {
      schema: "public",
      sequence_name: "users_id_seq",
      data_type: "bigint",
      start_value: 1,
      min_value: 1,
      max_value: 9223372036854775807,
      increment_by: 1,
      cycle: false,
      last_value: 100,
      table_name: "users",
      column_name: "id",
      percent_used: 0.0,
      remaining_count: 9223372036854775707,
      severity: "Normal",
    },
  ];

  assert.equal(
    sequencesSummaryText(seqs),
    "3 sequences · 1 critical · 1 warning",
  );

  const healthySeqs: SequenceRow[] = [seqs[2]!];
  assert.equal(sequencesSummaryText(healthySeqs), "1 sequence · all healthy");
});
