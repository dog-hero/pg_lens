// Unit tests for the SQL tokenizer — mirrors the TUI's test suite in
// crates/pg_lens_tui/src/ui/sql.rs so the two frontends stay in lockstep.
//
// No test framework: runs on Node >= 22.18 native TypeScript type-stripping
// and the built-in node:test runner (`npm test`). Vite never bundles this
// file (nothing imports it from the index.html graph).

import { test } from "node:test";
import assert from "node:assert/strict";

import { tokenizeSql, type SqlToken, type SqlTokenClass } from "./sql.ts";

/** Concatenated token text must always equal the input (styles only). */
function textOf(tokens: SqlToken[]): string {
  return tokens.map((t) => t.text).join("");
}

function ofClass(tokens: SqlToken[], cls: SqlTokenClass | null): string[] {
  return tokens.filter((t) => t.cls === cls).map((t) => t.text);
}

test("keywords highlight at start and middle, case-insensitively", () => {
  const sql = "select balance FROM accounts where id = 7";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "kw"), ["select", "FROM", "where"]);
  assert.deepEqual(ofClass(tokens, "num"), ["7"]);
});

test("quoted strings are green even when they contain keywords", () => {
  const sql = "SELECT 'DELETE FROM users' AS note";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "str"), ["'DELETE FROM users'"]);
  // DELETE/FROM inside the string must NOT appear as keyword tokens.
  assert.deepEqual(ofClass(tokens, "kw"), ["SELECT", "AS"]);
});

test("escaped quote stays inside the string", () => {
  const sql = "SELECT 'it''s' FROM t";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "str"), ["'it''s'"]);
  assert.deepEqual(ofClass(tokens, "kw"), ["SELECT", "FROM"]);
});

test("unterminated string swallows the rest without infinite loop", () => {
  const sql = "SELECT 'oops FROM t";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "str"), ["'oops FROM t"]);
});

test("digits inside identifiers and dollar params are not numbers", () => {
  const sql = "SELECT col1, $1 FROM t2 WHERE n = 42";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  // Only the standalone literal is a number token.
  assert.deepEqual(ofClass(tokens, "num"), ["42"]);
  // col1 / $1 / t2 stay plain (batched into unstyled runs).
  const plain = ofClass(tokens, null).join("");
  assert.ok(plain.includes("col1"));
  assert.ok(plain.includes("$1"));
  assert.ok(plain.includes("t2"));
});

test("keyword needs a word boundary", () => {
  // "selection" and "unfrom" and "FROMAGE" must not light up.
  const sql = "selection unfrom FROMAGE from x";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "kw"), ["from"]);
});

test("line comment dims to end of input", () => {
  const sql = "SELECT 1 -- DELETE everything 'later'";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "cmt"), ["-- DELETE everything 'later'"]);
  assert.deepEqual(ofClass(tokens, "kw"), ["SELECT"]);
});

test("decimal numbers and the truncation ellipsis survive", () => {
  const sql = "WHERE price > 19.99 AND abandoned_at < now() …";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "num"), ["19.99"]);
  assert.ok(ofClass(tokens, null).join("").includes("…"));
});

test("hostile query text stays inert text (XSS)", () => {
  // Query text comes from pg_stat_activity and is attacker-influenceable.
  // The tokenizer must pass markup through verbatim as plain text; the DOM
  // renderer only ever assigns it to textContent, never innerHTML.
  const sql = "SELECT '<img src=x onerror=alert(1)>' FROM \"<script>\" -- <b>";
  const tokens = tokenizeSql(sql);
  assert.equal(textOf(tokens), sql);
  assert.deepEqual(ofClass(tokens, "str"), ["'<img src=x onerror=alert(1)>'"]);
  assert.deepEqual(ofClass(tokens, "cmt"), ["-- <b>"]);
  // No token class ever carries markup semantics — every token is plain
  // text with one of five cls values.
  for (const t of tokens) {
    assert.ok([null, "kw", "str", "num", "cmt"].includes(t.cls));
  }
});
