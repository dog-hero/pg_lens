// Unit tests for the index advisor's (F3) severity/marker/description
// helpers — mirrors the Rust test suite in
// crates/pg_lens_core/src/index_advisor.rs so the two implementations stay
// in lockstep (same runner setup as waits.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  findingDescription,
  marker,
  partnerOf,
  severity,
  severityRank,
} from "./index-advisor.ts";
import type { IndexFinding } from "./types.ts";

test("severity maps every finding variant to its tier", () => {
  assert.equal(severity("Unused"), "unused");
  assert.equal(severity({ DuplicateExact: { partner: "p" } }), "dup");
  assert.equal(severity({ DuplicatePrefix: { partner: "p" } }), "prefix");
  assert.equal(severity("None"), "none");
});

test("severityRank orders Unused > DuplicateExact > DuplicatePrefix > None", () => {
  const findings: IndexFinding[] = [
    "Unused",
    { DuplicateExact: { partner: "p" } },
    { DuplicatePrefix: { partner: "p" } },
    "None",
  ];
  const ranks = findings.map(severityRank);
  assert.deepEqual(ranks, [0, 1, 2, 3]);
  // Strictly increasing — the sort in `IndexAdvisor.sorted` relies on this.
  for (let i = 1; i < ranks.length; i++) {
    assert.ok(ranks[i]! > ranks[i - 1]!);
  }
});

test("marker renders the plan's exact text: UNUSED / DUP / prefix / empty", () => {
  assert.equal(marker("Unused"), "UNUSED");
  assert.equal(marker({ DuplicateExact: { partner: "p" } }), "DUP");
  assert.equal(marker({ DuplicatePrefix: { partner: "p" } }), "prefix");
  assert.equal(marker("None"), "");
});

test("partnerOf extracts the duplicate's other index, null otherwise", () => {
  assert.equal(partnerOf({ DuplicateExact: { partner: "orders_pkey" } }), "orders_pkey");
  assert.equal(partnerOf({ DuplicatePrefix: { partner: "orders_wide_idx" } }), "orders_wide_idx");
  assert.equal(partnerOf("Unused"), null);
  assert.equal(partnerOf("None"), null);
});

test("findingDescription names the partner as evidence, not a bare label", () => {
  assert.match(findingDescription("Unused"), /UNUSED/);
  assert.match(findingDescription("Unused"), /zero scans/);
  const dup = findingDescription({ DuplicateExact: { partner: "orders_pkey" } });
  assert.match(dup, /orders_pkey/);
  assert.match(dup, /exact duplicate/);
  const prefix = findingDescription({ DuplicatePrefix: { partner: "orders_wide_idx" } });
  assert.match(prefix, /orders_wide_idx/);
  assert.match(prefix, /prefix/);
  assert.match(findingDescription("None"), /no finding/);
});
