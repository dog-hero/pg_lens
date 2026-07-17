// Micro Lens: activity table with client-side sort and B/W status markers.
//
// Mirrors the TUI's micro_lens.rs conventions: status column `S` shows `B`
// when the pid appears in DbSnapshot::locks (blocked — red tint, wins) and
// `W` when wait_event is non-null (waiting — yellow tint).

import type { ActivityRow, LockRow } from "./types";
import { humanDuration } from "./format";
import { renderSqlInto } from "./sql";
import { xactAgeSeverity } from "./xact_age";
import { blockingChain, renderBlockingChain } from "./blocking";
import type { AdminKind } from "./actions";

type SortKey =
  | "pid"
  | "database"
  | "username"
  | "client"
  | "state"
  | "wait_event"
  | "duration_secs"
  | "xact_age_secs"
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
  { key: "xact_age_secs", label: "Xact", numeric: true },
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
  private locks: LockRow[] = [];
  /** pid of the blocked row whose wait-for chain is expanded, if any —
   * mirrors the TUI's detail panel (v0.9), toggled by clicking a `B` row. */
  private expandedChainPid: number | null = null;
  private filter = "";
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly count: HTMLElement | null;
  /** True when admin actions are available (a token is active). */
  private readonly adminEnabled: () => boolean;
  /** Invoked when a row's Cancel/Kill button is pressed. */
  private readonly onAdmin: ((kind: AdminKind, row: ActivityRow) => void) | null;

  constructor(
    table: HTMLTableElement,
    filterInput?: HTMLInputElement | null,
    count?: HTMLElement | null,
    opts?: {
      adminEnabled?: () => boolean;
      onAdmin?: (kind: AdminKind, row: ActivityRow) => void;
    },
  ) {
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.count = count ?? null;
    this.adminEnabled = opts?.adminEnabled ?? (() => false);
    this.onAdmin = opts?.onAdmin ?? null;
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
    this.locks = locks;
    this.blocked = new Set(locks.map((lock) => lock.pid));
    // A pid can stop being blocked between polls (deadlock resolved, query
    // finished) — drop a stale expansion rather than showing a chain for a
    // pid that no longer has one.
    if (this.expandedChainPid !== null && !this.blocked.has(this.expandedChainPid)) {
      this.expandedChainPid = null;
    }
    // Re-render the head too: the Actions column appears once a token makes
    // admin available (it may become enabled after the first render).
    this.renderHead();
    this.renderBody();
  }

  private showActions(): boolean {
    return this.onAdmin !== null && this.adminEnabled();
  }

  /** Re-renders just the header — for when `adminEnabled()`'s answer can
   * change independently of a data update (e.g. `/api/config`'s read-only
   * flag resolving after the first snapshot already drew the Actions
   * column). Row data is untouched. */
  refreshHead(): void {
    this.renderHead();
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
    if (this.showActions()) {
      const th = document.createElement("th");
      th.textContent = "Actions";
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
      td.colSpan = COLUMNS.length + (this.showActions() ? 1 : 0);
      td.textContent =
        this.rows.length > 0 && this.filter
          ? `No sessions match “${this.filter}”`
          : "No active sessions";
      tr.append(td);
      this.tbody.replaceChildren(tr);
      return;
    }
    const colCount = COLUMNS.length + (this.showActions() ? 1 : 0);
    const trs: HTMLTableRowElement[] = [];
    for (const row of rows) {
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
      // Xact column: age of the open transaction ("—" when none), tinted
      // by the same severity the oldest-xact headline uses —
      // idle-in-transaction reads worse than an equally-old active query.
      const xactTd = document.createElement("td");
      xactTd.classList.add("num");
      if (row.xact_age_secs !== null) {
        xactTd.textContent = humanDuration(row.xact_age_secs);
        const severity = xactAgeSeverity(row.xact_age_secs, row.state);
        if (severity === "warn") xactTd.classList.add("xact-warn");
        else if (severity === "bad") xactTd.classList.add("xact-bad");
      } else {
        xactTd.textContent = "—";
        xactTd.classList.add("xact-none");
      }
      tr.append(xactTd);
      // Query cell: SQL-highlighted spans (XSS-safe — renderSqlInto only
      // ever writes textContent), tooltip carries the full text.
      const query = document.createElement("td");
      query.classList.add("query");
      query.title = row.query;
      renderSqlInto(query, row.query);
      tr.append(query);
      if (this.showActions()) {
        tr.append(this.actionsCell(row));
      }
      // v0.9: blocked rows are clickable — toggles the wait-for chain
      // (mirrors the TUI's Enter-to-open detail panel) into a sub-row
      // right below, so the reader gets to the root blocker without
      // leaving the table.
      if (isBlocked) {
        tr.classList.add("blocking-chain-toggle");
        tr.addEventListener("click", (e) => {
          // Don't hijack clicks on the Cancel/Kill buttons.
          if (e.target instanceof HTMLButtonElement) return;
          this.expandedChainPid = this.expandedChainPid === row.pid ? null : row.pid;
          this.renderBody();
        });
      }
      trs.push(tr);
      if (isBlocked && this.expandedChainPid === row.pid) {
        const chain = blockingChain(row.pid, this.locks);
        if (chain !== null) {
          const chainTr = document.createElement("tr");
          chainTr.classList.add("blocking-chain-row");
          const td = document.createElement("td");
          td.colSpan = colCount;
          td.append(renderBlockingChain(chain));
          chainTr.append(td);
          trs.push(chainTr);
        }
      }
    }
    this.tbody.replaceChildren(...trs);
  }

  /** Cancel / Kill buttons for one row (only rendered when admin is on). */
  private actionsCell(row: ActivityRow): HTMLTableCellElement {
    const td = document.createElement("td");
    td.classList.add("actions");
    const button = (kind: AdminKind, label: string, cls: string): HTMLButtonElement => {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = label;
      b.classList.add("action-btn", cls);
      b.addEventListener("click", () => this.onAdmin?.(kind, row));
      return b;
    };
    td.append(
      button("cancel", "Cancel", "cancel"),
      button("terminate", "Kill", "kill"),
    );
    return td;
  }
}
