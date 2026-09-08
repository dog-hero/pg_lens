import { test } from "node:test";
import assert from "node:assert/strict";

import {
  filterInputIdForPanel,
  isEditableTag,
  isExportKey,
  isHelpKey,
  isPaletteKey,
  isRecordKey,
  isSchemaRefreshKey,
  tabIdForKey,
} from "./keyboard.ts";

test("tabIdForKey maps 1-9 to the nine nav tabs in order", () => {
  assert.equal(tabIdForKey("1"), "tab-macro");
  assert.equal(tabIdForKey("2"), "tab-activity");
  assert.equal(tabIdForKey("3"), "tab-blocks");
  assert.equal(tabIdForKey("4"), "tab-replication");
  assert.equal(tabIdForKey("5"), "tab-schema");
  assert.equal(tabIdForKey("6"), "tab-indexes");
  assert.equal(tabIdForKey("7"), "tab-queries");
  assert.equal(tabIdForKey("8"), "tab-progress");
  assert.equal(tabIdForKey("9"), "tab-records");
});

test("tabIdForKey is null for anything outside 1-9", () => {
  assert.equal(tabIdForKey("0"), null);
  assert.equal(tabIdForKey("a"), null);
  assert.equal(tabIdForKey("/"), null);
});

test("filterInputIdForPanel resolves filterable panels", () => {
  assert.equal(filterInputIdForPanel("activity-panel"), "activity-filter");
  assert.equal(filterInputIdForPanel("replication-panel"), "replication-filter");
  assert.equal(filterInputIdForPanel("schema-panel"), "schema-filter");
  assert.equal(filterInputIdForPanel("indexes-panel"), "indexes-filter");
  assert.equal(filterInputIdForPanel("queries-panel"), "statements-filter");
  assert.equal(filterInputIdForPanel("progress-panel"), "progress-filter");
  assert.equal(filterInputIdForPanel("records-panel"), "records-filter");
});

test("filterInputIdForPanel is null for panels without a filter", () => {
  assert.equal(filterInputIdForPanel("macro-panel"), null);
  assert.equal(filterInputIdForPanel("nonexistent"), null);
});

test("isEditableTag flags text-consuming form elements only", () => {
  assert.equal(isEditableTag("INPUT"), true);
  assert.equal(isEditableTag("TEXTAREA"), true);
  assert.equal(isEditableTag("SELECT"), true);
  assert.equal(isEditableTag("BUTTON"), false);
  assert.equal(isEditableTag("DIV"), false);
});

test("isPaletteKey matches Cmd+K and Ctrl+K", () => {
  assert.equal(isPaletteKey({ key: "k", metaKey: true }), true);
  assert.equal(isPaletteKey({ key: "k", ctrlKey: true }), true);
  assert.equal(isPaletteKey({ key: "K", metaKey: true }), true);
  assert.equal(isPaletteKey({ key: "k" }), false);
  assert.equal(isPaletteKey({ key: "k", altKey: true }), false);
});

test("isRecordKey matches Shift+R ('R') and Ctrl+R", () => {
  assert.equal(isRecordKey({ key: "R" }), true);
  assert.equal(isRecordKey({ key: "r", ctrlKey: true }), true);
  assert.equal(isRecordKey({ key: "R", ctrlKey: true }), true);
  assert.equal(isRecordKey({ key: "r" }), false);
  assert.equal(isRecordKey({ key: "R", altKey: true }), false);
  assert.equal(isRecordKey({ key: "r", metaKey: true }), false);
});

test("isExportKey matches 'E' or 'e' without modifiers", () => {
  assert.equal(isExportKey({ key: "E" }), true);
  assert.equal(isExportKey({ key: "e" }), true);
  assert.equal(isExportKey({ key: "E", ctrlKey: true }), false);
  assert.equal(isExportKey({ key: "E", altKey: true }), false);
});

test("isSchemaRefreshKey matches 'B' or 'b' without modifiers", () => {
  assert.equal(isSchemaRefreshKey({ key: "B" }), true);
  assert.equal(isSchemaRefreshKey({ key: "b" }), true);
  assert.equal(isSchemaRefreshKey({ key: "B", ctrlKey: true }), false);
  assert.equal(isSchemaRefreshKey({ key: "B", altKey: true }), false);
});

test("isHelpKey matches '?'", () => {
  assert.equal(isHelpKey({ key: "?" }), true);
  assert.equal(isHelpKey({ key: "?", ctrlKey: true }), false);
  assert.equal(isHelpKey({ key: "h" }), false);
});
