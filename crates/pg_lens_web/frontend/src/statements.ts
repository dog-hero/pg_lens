// Query Lens: sortable pg_stat_statements table, mirroring the TUI's
// query_lens.rs conventions:
// - query cells are SQL-highlighted via sql.ts (textContent only — the
//   query text is attacker-influenceable);
// - Hit% = shared_blks_hit / (hit + read); `—` when zero blocks were
//   touched (never a made-up number);
// - Temp (v0.14) = temp_blks_written * 8kB — the work_mem spill signal;
//   `—` when zero, tinted (`.warn`) above TEMP_SPILL_WARN_BYTES; the rest
//   of the I/O & temp-spill profile (temp read, shared dirtied/written,
//   block read/write time, WAL bytes) lives in the detail row only;
// - staleness line `db: X · N statements · collected Xs ago · current
//   database only` that ticks locally between SSE frames (the collection
//   shares the Schema Lens slow cadence);
// - `Unavailable` (extension missing / older than 1.8) renders a friendly
//   explainer with the CREATE EXTENSION hint — a calm state, never the
//   error banner; a failed collection keeps the last rows under a warning;
// - clicking a row toggles a detail row: full normalized query
//   (highlighted) plus queryid/user/blocks.

import type { StatementRow, StatementsSnapshot } from "./types";
import { humanBytes, humanCount, humanDuration, humanMs } from "./format.ts";
import { renderSqlInto } from "./sql.ts";

const NO_HIT_RATIO = "—";
const NO_DATA = "—";

/** Postgres blocks are 8kB; `temp_blks_*` counters are in this unit. */
const BLOCK_BYTES = 8192;

/** Total temp-spill bytes above which the Temp cell tints `.warn` — any
 * nonzero spill is notable, but this is the "go look at this query's
 * work_mem" line. Mirrors the TUI's `TEMP_SPILL_WARN_BYTES` (100 MiB). */
const TEMP_SPILL_WARN_BYTES = 100 * 1024 * 1024;

/** Human bytes for a `temp_blks_written` count; `—` when zero (zero means
 * "this statement has not spilled", never a fabricated "0 B"). */
export function tempSpillCell(tempBlksWritten: number): string {
  return tempBlksWritten > 0 ? humanBytes(tempBlksWritten * BLOCK_BYTES) : NO_DATA;
}

/** `--` for a null I/O timing (track_io_timing off) or absent field
 * (e.g. wal_bytes on ext < 1.9) — mirrors the TUI's `NO_DATA` convention. */
function optionalCell(
  value: number | null,
  render: (v: number) => string,
): string {
  return value === null ? NO_DATA : render(value);
}

/** Hit% as a display string; `—` when no shared blocks were touched. */
export function hitPct(hit: number, read: number): string {
  const total = hit + read;
  if (total <= 0) return NO_HIT_RATIO;
  return `${((hit / total) * 100).toFixed(1)}%`;
}

type SortKey =
  | "query"
  | "calls"
  | "total_exec_ms"
  | "mean_exec_ms"
  | "rows"
  | "hit"
  | "temp_blks_written";

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
  { key: "temp_blks_written", label: "Temp", numeric: true },
];

/** Case-insensitive substring match over the normalized query text (and
 * queryid, if present) — mirrors the TUI's `statements_row_matches`
 * (v0.12). `needle` is already lowercased by the caller. */
export function statementsRowMatches(row: StatementRow, needle: string): boolean {
  return (
    row.query.toLowerCase().includes(needle) ||
    (row.query_id?.toLowerCase().includes(needle) ?? false)
  );
}

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
  /** v0.12: case-insensitive substring filter (query text / queryid),
   * mirroring the TUI's Query Lens `/` filter. Empty = no filter. */
  private filter = "";
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly staleness: HTMLElement;
  private readonly warning: HTMLElement;
  private readonly placeholder: HTMLElement;
  private readonly unavailable: HTMLElement;

  // Plain assignment, not TS constructor-parameter-property shorthand — see
  // `schema.ts`'s identical constructor doc comment for why.
  constructor(
    table: HTMLTableElement,
    staleness: HTMLElement,
    warning: HTMLElement,
    placeholder: HTMLElement,
    unavailable: HTMLElement,
    filterInput?: HTMLInputElement | null,
  ) {
    this.staleness = staleness;
    this.warning = warning;
    this.placeholder = placeholder;
    this.unavailable = unavailable;
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.renderHead();
    // Local 1s tick so "collected Xs ago" advances between SSE frames.
    setInterval(() => this.renderStaleness(), 1000);
    if (filterInput) {
      filterInput.addEventListener("input", () => {
        this.filter = filterInput.value.trim().toLowerCase();
        this.renderStaleness();
        this.renderBody();
      });
    }
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
    // v0.12: shown/total once a filter narrows the list — same fold-into-
    // the-staleness-line convention as the Schema Lens (no separate count
    // badge on this tab).
    const countText = this.filter
      ? `${s.statements.filter((r) => statementsRowMatches(r, this.filter)).length}/${s.statements.length} statements`
      : `${s.statements.length} statements`;
    this.staleness.textContent =
      `db: ${this.database} · ${countText} · ` +
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
    const visible = this.filter
      ? snapshot.statements.filter((r) => statementsRowMatches(r, this.filter))
      : snapshot.statements;
    return [...visible].sort((a, b) => {
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
    const tempTd = document.createElement("td");
    tempTd.textContent = tempSpillCell(row.temp_blks_written);
    tempTd.classList.add("num", "temp");
    if (row.temp_blks_written <= 0) {
      tempTd.title = "no temp-file spill";
    } else {
      tempTd.title = `temp read ${humanBytes(row.temp_blks_read * BLOCK_BYTES)} · written ${humanBytes(row.temp_blks_written * BLOCK_BYTES)}`;
      if (row.temp_blks_written * BLOCK_BYTES >= TEMP_SPILL_WARN_BYTES) {
        tempTd.classList.add("warn");
      }
    }
    tr.append(tempTd);
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
      `read ${humanCount(row.shared_blks_read)} / ` +
      `dirtied ${humanCount(row.shared_blks_dirtied)} / ` +
      `written ${humanCount(row.shared_blks_written)}`;
    // v0.14: I/O & temp-spill profile.
    const io = document.createElement("div");
    io.textContent =
      `temp spill: read ${tempSpillCell(row.temp_blks_read)} · ` +
      `written ${tempSpillCell(row.temp_blks_written)} · ` +
      `block I/O: read ${optionalCell(row.blk_read_time_ms, humanMs)} · ` +
      `write ${optionalCell(row.blk_write_time_ms, humanMs)} · ` +
      `WAL: ${optionalCell(row.wal_bytes, humanBytes)}`;
    if (row.temp_blks_written * BLOCK_BYTES >= TEMP_SPILL_WARN_BYTES) {
      io.classList.add("warn");
    }
    const query = document.createElement("pre");
    renderSqlInto(query, row.query);
    td.append(meta, io, query);
    tr.append(td);
    return tr;
  }
}
