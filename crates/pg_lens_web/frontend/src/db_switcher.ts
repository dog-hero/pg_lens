// Header database switcher (v0.13): web parity for the TUI's `d` database
// picker. PostgreSQL cannot switch databases in-session, so this posts to
// `/api/db/switch`, which asks the poller to reconnect with a different
// `dbname` — the next SSE snapshot then reflects the switch (no optimistic
// update here, same "let the stream refresh" contract the TUI uses after a
// db-picker Enter).
//
// Degrades gracefully when `databases` is null/absent (restricted role, or
// a single-database deployment): the dropdown hides itself and only the
// current database name is shown.

import type { DatabaseRow } from "./types";
import { humanBytes } from "./format.ts";

/** One `<option>`'s label: name plus a best-effort size, `"?"` when the
 * connected role couldn't read this OTHER database's size. */
export function dbOptionLabel(row: DatabaseRow): string {
  const size = row.size_bytes === null ? "?" : humanBytes(row.size_bytes);
  return `${row.name} (${size})`;
}

/**
 * Whether the switcher has anything useful to offer: `databases` is
 * non-null and lists at least two databases. A single-entry list (or a
 * restricted role that can't see `pg_database` at all, `null`) means there
 * is nothing to switch *to* — the dropdown degrades to "just show the
 * current db name" in both cases.
 */
export function hasSwitchableDatabases(databases: DatabaseRow[] | null): boolean {
  return databases !== null && databases.length >= 2;
}

/**
 * Rebuilds the `<select>`'s options from the current snapshot's `databases`
 * list, selecting `currentDb`. Leaves the element hidden (and untouched)
 * when [`hasSwitchableDatabases`] says there's nothing to switch between.
 */
export function populateDbSwitcher(
  select: HTMLSelectElement,
  databases: DatabaseRow[] | null,
  currentDb: string,
): void {
  if (!hasSwitchableDatabases(databases)) {
    select.hidden = true;
    return;
  }
  const options = (databases as DatabaseRow[]).map((row) => {
    const option = document.createElement("option");
    option.value = row.name;
    option.textContent = dbOptionLabel(row);
    option.selected = row.name === currentDb;
    return option;
  });
  select.replaceChildren(...options);
  select.hidden = false;
}
