import { test } from "node:test";
import assert from "node:assert/strict";

import { filterInputIdForPanel, isEditableTag, tabIdForKey } from "./keyboard.ts";

test("tabIdForKey maps 1-5 to the five nav tabs in order", () => {
  assert.equal(tabIdForKey("1"), "tab-activity");
  assert.equal(tabIdForKey("2"), "tab-replication");
  assert.equal(tabIdForKey("3"), "tab-schema");
  assert.equal(tabIdForKey("4"), "tab-indexes");
  assert.equal(tabIdForKey("5"), "tab-queries");
});

test("tabIdForKey is null for anything outside 1-5", () => {
  assert.equal(tabIdForKey("6"), null);
  assert.equal(tabIdForKey("a"), null);
  assert.equal(tabIdForKey("/"), null);
});

test("filterInputIdForPanel resolves the three filterable panels", () => {
  assert.equal(filterInputIdForPanel("activity-panel"), "activity-filter");
  assert.equal(filterInputIdForPanel("schema-panel"), "schema-filter");
  assert.equal(filterInputIdForPanel("queries-panel"), "statements-filter");
});

test("filterInputIdForPanel is null for panels without a filter", () => {
  assert.equal(filterInputIdForPanel("replication-panel"), null);
  assert.equal(filterInputIdForPanel("indexes-panel"), null);
  assert.equal(filterInputIdForPanel("nonexistent"), null);
});

test("isEditableTag flags text-consuming form elements only", () => {
  assert.equal(isEditableTag("INPUT"), true);
  assert.equal(isEditableTag("TEXTAREA"), true);
  assert.equal(isEditableTag("SELECT"), true);
  assert.equal(isEditableTag("BUTTON"), false);
  assert.equal(isEditableTag("DIV"), false);
});
