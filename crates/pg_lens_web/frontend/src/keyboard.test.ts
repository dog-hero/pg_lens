import { test } from "node:test";
import assert from "node:assert/strict";

import { filterInputIdForPanel, isEditableTag, isExportKey, isRecordKey, isSchemaRefreshKey, tabIdForKey } from "./keyboard.ts";

test("tabIdForKey maps 1-8 to the eight nav tabs in order", () => {
  assert.equal(tabIdForKey("1"), "tab-activity");
  assert.equal(tabIdForKey("2"), "tab-blocks");
  assert.equal(tabIdForKey("3"), "tab-replication");
  assert.equal(tabIdForKey("4"), "tab-schema");
  assert.equal(tabIdForKey("5"), "tab-indexes");
  assert.equal(tabIdForKey("6"), "tab-queries");
  assert.equal(tabIdForKey("7"), "tab-progress");
  assert.equal(tabIdForKey("8"), "tab-records");
});

test("tabIdForKey is null for anything outside 1-8", () => {
  assert.equal(tabIdForKey("9"), null);
  assert.equal(tabIdForKey("a"), null);
  assert.equal(tabIdForKey("/"), null);
});

test("filterInputIdForPanel resolves the six filterable panels", () => {
  assert.equal(filterInputIdForPanel("activity-panel"), "activity-filter");
  assert.equal(filterInputIdForPanel("schema-panel"), "schema-filter");
  assert.equal(filterInputIdForPanel("indexes-panel"), "indexes-filter");
  assert.equal(filterInputIdForPanel("queries-panel"), "statements-filter");
  assert.equal(filterInputIdForPanel("progress-panel"), "progress-filter");
  assert.equal(filterInputIdForPanel("records-panel"), "records-filter");
});

test("filterInputIdForPanel is null for panels without a filter", () => {
  assert.equal(filterInputIdForPanel("replication-panel"), null);
  assert.equal(filterInputIdForPanel("nonexistent"), null);
});

test("isEditableTag flags text-consuming form elements only", () => {
  assert.equal(isEditableTag("INPUT"), true);
  assert.equal(isEditableTag("TEXTAREA"), true);
  assert.equal(isEditableTag("SELECT"), true);
  assert.equal(isEditableTag("BUTTON"), false);
  assert.equal(isEditableTag("DIV"), false);
});

test("isRecordKey matches Shift+R ('R') and Ctrl+R", () => {
  assert.equal(isRecordKey({ key: "R" }), true);
  assert.equal(isRecordKey({ key: "r", ctrlKey: true }), true);
  assert.equal(isRecordKey({ key: "R", ctrlKey: true }), true);
  assert.equal(isRecordKey({ key: "r" }), false);
  assert.equal(isRecordKey({ key: "R", altKey: true }), false);
  assert.equal(isRecordKey({ key: "r", metaKey: true }), false);
});

test("isExportKey matches 'E' without modifiers", () => {
  assert.equal(isExportKey({ key: "E" }), true);
  assert.equal(isExportKey({ key: "e" }), false);
  assert.equal(isExportKey({ key: "E", ctrlKey: true }), false);
  assert.equal(isExportKey({ key: "E", altKey: true }), false);
});

test("isSchemaRefreshKey matches 'B' without modifiers", () => {
  assert.equal(isSchemaRefreshKey({ key: "B" }), true);
  assert.equal(isSchemaRefreshKey({ key: "b" }), false);
  assert.equal(isSchemaRefreshKey({ key: "B", ctrlKey: true }), false);
  assert.equal(isSchemaRefreshKey({ key: "B", altKey: true }), false);
});
