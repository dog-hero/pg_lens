// Query Lens: sortable pg_stat_statements table, mirroring the TUI's
// query_lens.rs conventions:
// - query cells are SQL-highlighted via sql.ts (textContent only — the
//   query text is attacker-influenceable);
// - Hit% = shared_blks_hit / (hit + read); `—` when zero blocks were
//   touched (never a made-up number);
// - staleness line `db: X · N statements · collected Xs ago · current
//   database only` that ticks locally between SSE frames (the collection
//   shares the Schema Lens slow cadence);
// - `Unavailable` (extension missing / older than 1.8) renders a friendly
//   explainer with the CREATE EXTENSION hint — a calm state, never the
//   error banner; a failed collection keeps the last rows under a warning;
// - clicking a row toggles a detail row: full normalized query
//   (highlighted) plus queryid/user/blocks.

import type { StatementRow, StatementsSnapshot } from "./types";
import { humanCount, humanDuration } from "./format";
import { renderSqlInto } from "./sql";

const NO_HIT_RATIO = "—";

/** Hit% as a display string; `—` when no shared blocks were touched. */
export function hitPct(hit: number, read: number): string {
  const total = hit + read;
  if (total <= 0) return NO_HIT_RATIO;
  return `${((hit / total) * 100).toFixed(1)}%`;
}

/** `189442.7ms` → `3m09s`-style: pg_stat_statements times are in ms. */
function humanMs(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0ms";
  if (ms < 1) return `${ms.toFixed(2)}ms`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return humanDuration(ms / 1000);
}

type SortKey = "query" | "calls" | "total_exec_ms" | "mean_exec_ms" | "rows" | "hit";

interface Column {
  key: SortKey;
  label: string;
  numeric: boolean;
}

const COLUMNS: Column[] = [
  { key: "query", label: "Query", numeric: false },
  { key: "calls", label: "Calls", numeric: true },
  { key: "total_exec_ms", label: "Total", numeric: true },
  { key: "mean_exec_ms", label: "Mean", numeric: true },
  { key: "rows", label: "Rows", numeric: true },
  { key: "hit", label: "Hit %", numeric: true },
];

/** Value each sortable column orders by (no-ratio rows sort last). */
function sortValue(key: SortKey, row: StatementRow): number | string {
  switch (key) {
    case "query":
      return row.query;
    case "hit": {
      const total = row.shared_blks_hit + row.shared_blks_read;
      return total > 0 ? row.shared_blks_hit / total : -1;
    }
    default:
      return row[key];
  }
}

export class StatementsLens {
  private sortKey: SortKey = "total_exec_ms";
  private sortAsc = false;
  private snapshot: StatementsSnapshot | null = null;
  private database = "";
  /** `query_id ?? query` keys of rows whose detail is open. */
  private readonly expanded = new Set<string>();
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;

  constructor(
    table: HTMLTableElement,
    private readonly staleness: HTMLElement,
    private readonly warning: HTMLElement,
    private readonly placeholder: HTMLElement,
    private readonly unavailable: HTMLElement,
  ) {
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.renderHead();
    // Local 1s tick so "collected Xs ago" advances between SSE frames.
    setInterval(() => this.renderStaleness(), 1000);
  }

  update(statements: StatementsSnapshot | null, database: string): void {
    const changed =
      statements?.collected_at_epoch_ms !==
      this.snapshot?.collected_at_epoch_ms;
    this.snapshot = statements;
    this.database = database;
    const unavailableReason = this.unavailableReason();
    this.placeholder.hidden = statements !== null;
    this.unavailable.hidden = unavailableReason === null;
    if (unavailableReason !== null) {
      this.renderUnavailable(unavailableReason);
      this.staleness.textContent = "";
      this.warning.hidden = true;
      this.tbody.replaceChildren();
      return;
    }
    this.renderStaleness();
    this.renderWarning();
    if (changed) this.renderBody();
  }

  private unavailableReason(): string | null {
    const status = this.snapshot?.status;
    if (typeof status === "object" && "Unavailable" in status) {
      return status.Unavailable;
    }
    return null;
  }

  /** Friendly explainer: the reason plus the exact SQL to run. */
  private renderUnavailable(reason: string): void {
    this.unavailable.replaceChildren();
    const title = document.createElement("h3");
    title.textContent = "pg_stat_statements not available";
    const why = document.createElement("p");
    why.textContent = reason;
    const how = document.createElement("p");
    how.append(document.createTextNode("To enable it: "));
    const code = document.createElement("code");
    renderSqlInto(code, "CREATE EXTENSION pg_stat_statements;");
    how.append(code);
    how.append(
      document.createTextNode(
        " (needs shared_preload_libraries = 'pg_stat_statements' and a restart)",
      ),
    );
    this.unavailable.append(title, why, how);
  }

  private setSort(key: SortKey): void {
    if (this.sortKey === key) {
      this.sortAsc = !this.sortAsc;
    } else {
      this.sortKey = key;
      // Numbers want "biggest first" on first click; the query column A→Z.
      this.sortAsc = key === "query";
    }
    this.renderHead();
    this.renderBody();
  }

  private renderHead(): void {
    const tr = document.createElement("tr");
    for (const col of COLUMNS) {
      const th = document.createElement("th");
      th.textContent = col.label;
      const key = col.key;
      th.classList.add("sortable");
      if (key === this.sortKey) {
        th.classList.add("sorted");
        th.textContent = `${col.label} ${this.sortAsc ? "▲" : "▼"}`;
      }
      th.addEventListener("click", () => this.setSort(key));
      if (col.numeric) th.classList.add("num");
      tr.append(th);
    }
    this.thead.replaceChildren(tr);
  }

  private renderStaleness(): void {
    const s = this.snapshot;
    if (s === null || this.unavailableReason() !== null) {
      this.staleness.textContent = "";
      return;
    }
    const ageSecs = Math.max(0, (Date.now() - s.collected_at_epoch_ms) / 1000);
    this.staleness.textContent =
      `db: ${this.database} · ${s.statements.length} statements · ` +
      `collected ${humanDuration(ageSecs)} ago · current database only`;
  }

  private renderWarning(): void {
    const status = this.snapshot?.status;
    if (typeof status === "object" && "Error" in status) {
      this.warning.textContent = `statements: ${status.Error} — showing last collection`;
      this.warning.hidden = false;
    } else {
      this.warning.hidden = true;
    }
  }

  private sorted(snapshot: StatementsSnapshot): StatementRow[] {
    const key = this.sortKey;
    const dir = this.sortAsc ? 1 : -1;
    return [...snapshot.statements].sort((a, b) => {
      const va = sortValue(key, a);
      const vb = sortValue(key, b);
      if (typeof va === "number" && typeof vb === "number") {
        return (va - vb) * dir;
      }
      return String(va).localeCompare(String(vb)) * dir;
    });
  }

  private renderBody(): void {
    const snapshot = this.snapshot;
    if (snapshot === null) {
      this.tbody.replaceChildren();
      return;
    }
    const rows: HTMLTableRowElement[] = [];
    for (const row of this.sorted(snapshot)) {
      const rowKey = row.query_id ?? row.query;
      rows.push(this.dataRow(row, rowKey));
      if (this.expanded.has(rowKey)) {
        rows.push(this.detailRow(row));
      }
    }
    this.tbody.replaceChildren(...rows);
  }

  private dataRow(row: StatementRow, rowKey: string): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("statement-row");

    const queryTd = document.createElement("td");
    queryTd.classList.add("query");
    renderSqlInto(queryTd, row.query);
    queryTd.title = row.query;
    tr.append(queryTd);

    const cells: string[] = [
      humanCount(row.calls),
      humanMs(row.total_exec_ms),
      humanMs(row.mean_exec_ms),
      humanCount(row.rows),
      hitPct(row.shared_blks_hit, row.shared_blks_read),
    ];
    for (const text of cells) {
      const td = document.createElement("td");
      td.textContent = text;
      td.classList.add("num");
      if (text === NO_HIT_RATIO) td.title = "no shared blocks touched";
      tr.append(td);
    }
    tr.title = "click for the full query + detail";
    tr.addEventListener("click", () => {
      if (this.expanded.has(rowKey)) this.expanded.delete(rowKey);
      else this.expanded.add(rowKey);
      this.renderBody();
    });
    return tr;
  }

  /** Full-width detail row: highlighted full query + remaining metrics. */
  private detailRow(row: StatementRow): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("statement-detail");
    const td = document.createElement("td");
    td.colSpan = COLUMNS.length;
    const meta = document.createElement("div");
    meta.textContent =
      `queryid ${row.query_id ?? "—"} · user ${row.username} · ` +
      `shared blocks hit ${humanCount(row.shared_blks_hit)} / ` +
      `read ${humanCount(row.shared_blks_read)}`;
    const query = document.createElement("pre");
    renderSqlInto(query, row.query);
    td.append(meta, query);
    tr.append(td);
    return tr;
  }
}
