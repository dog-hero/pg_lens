import { test } from "node:test";
import assert from "node:assert/strict";

import { filterInputIdForPanel, isEditableTag, tabIdForKey } from "./keyboard.ts";

test("tabIdForKey maps 1-7 to the seven nav tabs in order", () => {
  assert.equal(tabIdForKey("1"), "tab-activity");
  assert.equal(tabIdForKey("2"), "tab-blocks");
  assert.equal(tabIdForKey("3"), "tab-replication");
  assert.equal(tabIdForKey("4"), "tab-schema");
  assert.equal(tabIdForKey("5"), "tab-indexes");
  assert.equal(tabIdForKey("6"), "tab-queries");
  assert.equal(tabIdForKey("7"), "tab-progress");
});

test("tabIdForKey is null for anything outside 1-7", () => {
  assert.equal(tabIdForKey("8"), null);
  assert.equal(tabIdForKey("a"), null);
  assert.equal(tabIdForKey("/"), null);
});

test("filterInputIdForPanel resolves the four filterable panels", () => {
  assert.equal(filterInputIdForPanel("activity-panel"), "activity-filter");
  assert.equal(filterInputIdForPanel("schema-panel"), "schema-filter");
  assert.equal(filterInputIdForPanel("queries-panel"), "statements-filter");
  assert.equal(filterInputIdForPanel("progress-panel"), "progress-filter");
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
