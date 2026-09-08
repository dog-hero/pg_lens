// Micro Lens: activity table with client-side sort, state filters, and keyboard navigation.
// Integrates with InspectorDrawer for smooth side-by-side inspection.

import type { ActivityRow, LockRow } from "./types";
import { humanDuration } from "./format.ts";
import { xactAgeSeverity } from "./xact_age.ts";
import type { AdminKind } from "./actions.ts";

export const ROW_DURATION_BAD_SECS = 30;
export const ROW_DURATION_WARN_SECS = 10;

export function stateColorClass(state: string): string {
  switch (state) {
    case "active":
      return "state-active";
    case "idle":
      return "state-idle";
    case "idle in transaction":
      return "state-idle-txn";
    case "idle in transaction (aborted)":
      return "state-idle-txn-aborted";
    default:
      return "";
  }
}

export function durationSeverityClass(
  state: string,
  durationSecs: number,
): string {
  if (state === "active") {
    if (durationSecs > ROW_DURATION_BAD_SECS) return "duration-bad";
    if (durationSecs > ROW_DURATION_WARN_SECS) return "duration-warn";
    return "duration-ok";
  }
  return "duration-idle";
}

export function waitEventClass(waitEvent: string | null): string {
  if (!waitEvent) return "wait-none";
  if (waitEvent.startsWith("Lock:")) return "wait-lock";
  return "wait-other";
}

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

export type StateFilter = "all" | "active" | "waiting" | "idle_txn" | "blocked";

export class ActivityTable {
  private sortKey: SortKey = "duration_secs";
  private sortAsc = false;
  private rows: ActivityRow[] = [];
  private blocked = new Set<number>();
  private locks: LockRow[] = [];
  private selectedPid: number | null = null;
  private filter = "";
  private stateFilter: StateFilter = "all";

  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly count: HTMLElement | null;
  private readonly adminEnabled: () => boolean;
  private readonly onAdmin: ((kind: AdminKind, row: ActivityRow) => void) | null;
  private readonly onInspect: ((row: ActivityRow, locks: LockRow[]) => void) | null;

  constructor(
    table: HTMLTableElement,
    filterInput?: HTMLInputElement | null,
    count?: HTMLElement | null,
    opts?: {
      adminEnabled?: () => boolean;
      onAdmin?: (kind: AdminKind, row: ActivityRow) => void;
      onInspect?: (row: ActivityRow, locks: LockRow[]) => void;
    },
  ) {
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.count = count ?? null;
    this.adminEnabled = opts?.adminEnabled ?? (() => false);
    this.onAdmin = opts?.onAdmin ?? null;
    this.onInspect = opts?.onInspect ?? null;

    if (filterInput) {
      filterInput.addEventListener("input", () => {
        this.filter = filterInput.value.trim().toLowerCase();
        this.renderBody();
      });
    }
    this.renderHead();
  }

  setStateFilter(filter: StateFilter): void {
    this.stateFilter = filter;
    this.renderBody();
  }

  getStateFilter(): StateFilter {
    return this.stateFilter;
  }

  update(activity: ActivityRow[], locks: LockRow[], _ddlProgress?: unknown): void {
    this.rows = activity;
    this.locks = locks;
    this.blocked = new Set(locks.map((lock) => lock.pid));

    if (this.selectedPid !== null && !this.rows.some((r) => r.pid === this.selectedPid)) {
      this.selectedPid = null;
    }

    this.renderHead();
    this.renderBody();
  }

  getSelectedRow(): ActivityRow | null {
    if (this.selectedPid === null) return null;
    return this.rows.find((r) => r.pid === this.selectedPid) ?? null;
  }

  selectNext(): void {
    const visible = this.sorted();
    const first = visible[0];
    if (!first) return;
    if (this.selectedPid === null) {
      this.selectedPid = first.pid;
    } else {
      const idx = visible.findIndex((r) => r.pid === this.selectedPid);
      if (idx === -1 || idx === visible.length - 1) {
        this.selectedPid = first.pid;
      } else {
        const next = visible[idx + 1];
        if (next) this.selectedPid = next.pid;
      }
    }
    this.renderBody();
    this.scrollSelectedIntoView();
  }

  selectPrev(): void {
    const visible = this.sorted();
    const last = visible[visible.length - 1];
    if (!last) return;
    if (this.selectedPid === null) {
      this.selectedPid = last.pid;
    } else {
      const idx = visible.findIndex((r) => r.pid === this.selectedPid);
      if (idx <= 0) {
        this.selectedPid = last.pid;
      } else {
        const prev = visible[idx - 1];
        if (prev) this.selectedPid = prev.pid;
      }
    }
    this.renderBody();
    this.scrollSelectedIntoView();
  }

  inspectSelected(): void {
    const sel = this.getSelectedRow();
    if (sel && this.onInspect) {
      this.onInspect(sel, this.locks);
    }
  }

  private scrollSelectedIntoView(): void {
    const tr = this.tbody.querySelector(`tr[data-pid="${this.selectedPid}"]`);
    if (tr instanceof HTMLElement) {
      tr.scrollIntoView({ block: "nearest" });
    }
  }

  private showActions(): boolean {
    return this.onAdmin !== null && this.adminEnabled();
  }

  refreshHead(): void {
    this.renderHead();
  }

  private setSort(key: SortKey): void {
    if (this.sortKey === key) {
      this.sortAsc = !this.sortAsc;
    } else {
      this.sortKey = key;
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

    let filtered = this.rows;

    // Apply text search
    if (this.filter) {
      filtered = filtered.filter((r) => rowMatches(r, this.filter));
    }

    // Apply state filter chip
    if (this.stateFilter === "active") {
      filtered = filtered.filter((r) => r.state === "active");
    } else if (this.stateFilter === "waiting") {
      filtered = filtered.filter((r) => r.wait_event !== null);
    } else if (this.stateFilter === "idle_txn") {
      filtered = filtered.filter((r) => r.state.includes("idle in transaction"));
    } else if (this.stateFilter === "blocked") {
      filtered = filtered.filter((r) => this.blocked.has(r.pid));
    }

    if (this.count) {
      this.count.textContent =
        this.filter || this.stateFilter !== "all"
          ? `${filtered.length}/${this.rows.length}`
          : `${this.rows.length}`;
    }

    return [...filtered].sort((a, b) => {
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
      const tr = document.createElement("tr");
      tr.classList.add("empty-row");
      const td = document.createElement("td");
      td.colSpan = COLUMNS.length + (this.showActions() ? 1 : 0);
      td.textContent =
        this.rows.length > 0
          ? `No sessions match current filter`
          : "No active sessions";
      tr.append(td);
      this.tbody.replaceChildren(tr);
      return;
    }

    const trs: HTMLTableRowElement[] = [];
    for (const row of rows) {
      const isBlocked = this.blocked.has(row.pid);
      const isWaiting = row.wait_event !== null;
      const isSelected = this.selectedPid === row.pid;

      const tr = document.createElement("tr");
      tr.dataset.pid = String(row.pid);
      if (isSelected) tr.classList.add("is-selected");

      // Status column
      const statusTd = document.createElement("td");
      statusTd.classList.add("col-status");
      const marker = isBlocked ? "B" : isWaiting ? "W" : "";
      if (isBlocked) {
        statusTd.classList.add("status-blocked");
      } else if (isWaiting) {
        statusTd.classList.add("status-waiting");
      }
      statusTd.textContent = marker;
      tr.append(statusTd);

      // PID column
      const pidTd = document.createElement("td");
      pidTd.classList.add("col-pid", "num");
      if (isBlocked) {
        pidTd.classList.add("pid-blocked");
      }
      pidTd.textContent = String(row.pid);
      tr.append(pidTd);

      // Database column
      const dbTd = document.createElement("td");
      dbTd.classList.add("col-db");
      dbTd.textContent = row.database;
      tr.append(dbTd);

      // Username column
      const userTd = document.createElement("td");
      userTd.classList.add("col-user");
      userTd.textContent = row.username;
      tr.append(userTd);

      // Client column
      const clientTd = document.createElement("td");
      clientTd.classList.add("col-client");
      if (row.ssl) {
        const badge = document.createElement("span");
        badge.className = "ssl-badge";
        badge.title = `SSL: ${row.ssl_version ?? "TLS"} (${row.ssl_cipher ?? "encrypted"})`;
        badge.textContent = "🔒 ";
        clientTd.append(badge);
      }
      clientTd.append(document.createTextNode(row.client));
      tr.append(clientTd);

      // State column
      const stateTd = document.createElement("td");
      stateTd.classList.add("col-state");
      const sClass = stateColorClass(row.state);
      if (sClass) stateTd.classList.add(sClass);
      stateTd.textContent = row.state;
      tr.append(stateTd);

      // Wait column
      const waitTd = document.createElement("td");
      waitTd.classList.add("col-wait");
      waitTd.classList.add(waitEventClass(row.wait_event));
      waitTd.textContent = row.wait_event ?? "—";
      tr.append(waitTd);

      // Duration column
      const durationTd = document.createElement("td");
      durationTd.classList.add("col-duration", "num");
      durationTd.classList.add(durationSeverityClass(row.state, row.duration_secs));
      durationTd.textContent = humanDuration(row.duration_secs);
      tr.append(durationTd);

      // Xact column
      const xactTd = document.createElement("td");
      xactTd.classList.add("col-xact", "num");
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

      // Query cell
      const query = document.createElement("td");
      query.classList.add("col-query", "query");
      query.title = row.query;
      query.textContent = row.query;
      tr.append(query);

      if (this.showActions()) {
        tr.append(this.actionsCell(row));
      }

      // Clicking row selects it and opens InspectorDrawer
      tr.addEventListener("click", (e) => {
        if (e.target instanceof HTMLButtonElement) return;
        this.selectedPid = row.pid;
        this.renderSelectionClass();
        this.onInspect?.(row, this.locks);
      });

      trs.push(tr);
    }
    this.tbody.replaceChildren(...trs);
  }

  private renderSelectionClass(): void {
    for (const tr of this.tbody.querySelectorAll("tr")) {
      const pid = tr.dataset.pid;
      if (pid && Number(pid) === this.selectedPid) {
        tr.classList.add("is-selected");
      } else {
        tr.classList.remove("is-selected");
      }
    }
  }

  private actionsCell(row: ActivityRow): HTMLTableCellElement {
    const td = document.createElement("td");
    td.classList.add("col-actions");

    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "cell-btn btn-cancel";
    cancel.textContent = "Cancel";
    cancel.title = `Cancel the running query for pid ${row.pid}`;
    cancel.addEventListener("click", (e) => {
      e.stopPropagation();
      this.onAdmin?.("cancel", row);
    });

    const terminate = document.createElement("button");
    terminate.type = "button";
    terminate.className = "cell-btn btn-kill";
    terminate.textContent = "Kill";
    terminate.title = `Terminate the backend connection for pid ${row.pid}`;
    terminate.addEventListener("click", (e) => {
      e.stopPropagation();
      this.onAdmin?.("terminate", row);
    });

    td.append(cancel, " ", terminate);
    return td;
  }
}
