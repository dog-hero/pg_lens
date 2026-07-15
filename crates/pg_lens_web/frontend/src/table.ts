// Micro Lens: activity table with client-side sort and B/W status markers.
//
// Mirrors the TUI's micro_lens.rs conventions: status column `S` shows `B`
// when the pid appears in DbSnapshot::locks (blocked — red tint, wins) and
// `W` when wait_event is non-null (waiting — yellow tint).

import type { ActivityRow, LockRow } from "./types";
import { humanDuration } from "./format";
import { renderSqlInto } from "./sql";

type SortKey =
  | "pid"
  | "database"
  | "username"
  | "client"
  | "state"
  | "wait_event"
  | "duration_secs"
  | "query";

interface Column {
  key: SortKey | "status";
  label: string;
  numeric: boolean;
}

const COLUMNS: Column[] = [
  { key: "status", label: "S", numeric: false },
  { key: "pid", label: "PID", numeric: true },
  { key: "database", label: "DB", numeric: false },
  { key: "username", label: "User", numeric: false },
  { key: "client", label: "Client", numeric: false },
  { key: "state", label: "State", numeric: false },
  { key: "wait_event", label: "Wait", numeric: false },
  { key: "duration_secs", label: "Duration", numeric: true },
  { key: "query", label: "Query", numeric: false },
];

/** Case-insensitive substring match over the fields a DBA filters by —
 * mirrors the TUI's `row_matches` (pid as text, everything else a contains). */
function rowMatches(row: ActivityRow, needle: string): boolean {
  return (
    String(row.pid).includes(needle) ||
    row.database.toLowerCase().includes(needle) ||
    row.username.toLowerCase().includes(needle) ||
    row.application_name.toLowerCase().includes(needle) ||
    row.client.toLowerCase().includes(needle) ||
    row.state.toLowerCase().includes(needle) ||
    (row.wait_event?.toLowerCase().includes(needle) ?? false) ||
    row.query.toLowerCase().includes(needle)
  );
}

export class ActivityTable {
  private sortKey: SortKey = "duration_secs";
  private sortAsc = false;
  private rows: ActivityRow[] = [];
  private blocked = new Set<number>();
  private filter = "";
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly count: HTMLElement | null;

  constructor(
    table: HTMLTableElement,
    filterInput?: HTMLInputElement | null,
    count?: HTMLElement | null,
  ) {
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.count = count ?? null;
    if (filterInput) {
      filterInput.addEventListener("input", () => {
        this.filter = filterInput.value.trim().toLowerCase();
        this.renderBody();
      });
    }
    this.renderHead();
  }

  update(activity: ActivityRow[], locks: LockRow[]): void {
    this.rows = activity;
    this.blocked = new Set(locks.map((lock) => lock.pid));
    this.renderBody();
  }

  private setSort(key: SortKey): void {
    if (this.sortKey === key) {
      this.sortAsc = !this.sortAsc;
    } else {
      this.sortKey = key;
      // Numbers usually want "biggest first" on first click; text A→Z.
      this.sortAsc = !(key === "duration_secs" || key === "pid");
    }
    this.renderHead();
    this.renderBody();
  }

  private renderHead(): void {
    const tr = document.createElement("tr");
    for (const col of COLUMNS) {
      const th = document.createElement("th");
      th.textContent = col.label;
      if (col.key !== "status") {
        const key = col.key;
        th.classList.add("sortable");
        if (key === this.sortKey) {
          th.classList.add("sorted");
          th.textContent = `${col.label} ${this.sortAsc ? "▲" : "▼"}`;
        }
        th.addEventListener("click", () => this.setSort(key));
      }
      if (col.numeric) th.classList.add("num");
      tr.append(th);
    }
    this.thead.replaceChildren(tr);
  }

  private sorted(): ActivityRow[] {
    const key = this.sortKey;
    const dir = this.sortAsc ? 1 : -1;
    const visible = this.filter
      ? this.rows.filter((r) => rowMatches(r, this.filter))
      : this.rows;
    if (this.count) {
      this.count.textContent = this.filter
        ? `${visible.length}/${this.rows.length}`
        : `${this.rows.length}`;
    }
    return [...visible].sort((a, b) => {
      const va = a[key] ?? "";
      const vb = b[key] ?? "";
      if (typeof va === "number" && typeof vb === "number") {
        return (va - vb) * dir;
      }
      return String(va).localeCompare(String(vb)) * dir;
    });
  }

  private renderBody(): void {
    const rows = this.sorted();
    if (rows.length === 0) {
      // Empty state: distinguish "nothing matches your filter" from "the
      // server is genuinely idle" so the reader knows which lever to pull.
      const tr = document.createElement("tr");
      tr.classList.add("empty-row");
      const td = document.createElement("td");
      td.colSpan = COLUMNS.length;
      td.textContent =
        this.rows.length > 0 && this.filter
          ? `No sessions match “${this.filter}”`
          : "No active sessions";
      tr.append(td);
      this.tbody.replaceChildren(tr);
      return;
    }
    this.tbody.replaceChildren(
      ...rows.map((row) => {
        const isBlocked = this.blocked.has(row.pid);
        const isWaiting = row.wait_event !== null;
        const tr = document.createElement("tr");
        if (isBlocked) tr.classList.add("blocked");
        else if (isWaiting) tr.classList.add("waiting");
        const marker = isBlocked ? "B" : isWaiting ? "W" : "";
        const cells: Array<[string, boolean]> = [
          [marker, false],
          [String(row.pid), true],
          [row.database, false],
          [row.username, false],
          [row.client, false],
          [row.state, false],
          [row.wait_event ?? "", false],
          [humanDuration(row.duration_secs), true],
        ];
        for (const [text, numeric] of cells) {
          const td = document.createElement("td");
          td.textContent = text;
          if (numeric) td.classList.add("num");
          tr.append(td);
        }
        // Query cell: SQL-highlighted spans (XSS-safe — renderSqlInto only
        // ever writes textContent), tooltip carries the full text.
        const query = document.createElement("td");
        query.classList.add("query");
        query.title = row.query;
        renderSqlInto(query, row.query);
        tr.append(query);
        return tr;
      }),
    );
  }
}
