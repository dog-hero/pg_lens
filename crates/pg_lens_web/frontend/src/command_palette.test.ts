import { test } from "node:test";
import assert from "node:assert/strict";

import { filterPaletteActions, type PaletteAction } from "./command_palette.ts";

const sampleActions: PaletteAction[] = [
  { id: "nav-macro", group: "Lenses", label: "Macro Overview & Saturation", shortcut: "1", run: () => {} },
  { id: "nav-activity", group: "Lenses", label: "Micro Activity & Live Sessions", shortcut: "2", run: () => {} },
  { id: "nav-blocks", group: "Lenses", label: "Block & Lock-Wait Tree", shortcut: "3", run: () => {} },
  { id: "nav-replication", group: "Lenses", label: "Replication Topology & Slots", shortcut: "4", run: () => {} },
  { id: "db-postgres", group: "Databases", label: "Switch database: postgres", detail: "15.2 MB", run: () => {} },
  { id: "db-analytics", group: "Databases", label: "Switch database: analytics", detail: "4.8 GB", run: () => {} },
  { id: "act-pause", group: "Actions", label: "Pause live stream", shortcut: "Space", run: () => {} },
  { id: "act-record", group: "Actions", label: "Start incident recording", shortcut: "Shift+R", run: () => {} },
];

test("filterPaletteActions returns all actions when query is empty or whitespace", () => {
  assert.equal(filterPaletteActions("", sampleActions).length, sampleActions.length);
  assert.equal(filterPaletteActions("   ", sampleActions).length, sampleActions.length);
});

test("filterPaletteActions matches by label case-insensitively", () => {
  const res = filterPaletteActions("macro", sampleActions);
  assert.equal(res.length, 1);
  assert.equal(res[0]?.id, "nav-macro");

  const res2 = filterPaletteActions("REPLICATION", sampleActions);
  assert.equal(res2.length, 1);
  assert.equal(res2[0]?.id, "nav-replication");
});

test("filterPaletteActions matches by group name", () => {
  const databases = filterPaletteActions("database", sampleActions);
  assert.equal(databases.length, 2);
  assert.ok(databases.every((d) => d.group === "Databases"));

  const lenses = filterPaletteActions("lenses", sampleActions);
  assert.equal(lenses.length, 4);
});

test("filterPaletteActions matches by detail field", () => {
  const mb = filterPaletteActions("15.2", sampleActions);
  assert.equal(mb.length, 1);
  assert.equal(mb[0]?.id, "db-postgres");

  const gb = filterPaletteActions("4.8 GB", sampleActions);
  assert.equal(gb.length, 1);
  assert.equal(gb[0]?.id, "db-analytics");
});

test("filterPaletteActions returns empty array on unmatched search", () => {
  const res = filterPaletteActions("nonexistent query term 123", sampleActions);
  assert.equal(res.length, 0);
});
