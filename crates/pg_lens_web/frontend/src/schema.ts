// Schema Lens: sortable per-table stats + estimated bloat, mirroring the
// TUI's schema_lens.rs conventions:
// - severity column `!`: `!!` = red tier (estimated bloat% > 50 AND
//   bloat > 10 MiB), `!` = yellow tier (> 30% AND > 1 MiB) — red wins;
// - `~?` wherever the estimate is not applicable (`is_na`) or missing,
//   with a tooltip explaining why — never a made-up number;
// - staleness line `db: X · N tables · collected Xs ago · estimated bloat`
//   that ticks locally between SSE updates and resets when a new
//   `collected_at_epoch_ms` arrives;
// - clicking a row toggles a detail row listing that table's index bloat.

import type { BloatRow, SchemaSnapshot, TableStatRow } from "./types";
import { humanAgo, humanBytes, humanCount, humanDuration } from "./format.ts";

const NO_ESTIMATE = "~?";
const NO_ESTIMATE_TITLE = "estimated (needs fresh ANALYZE)";

type Severity = "red" | "yellow" | "na" | "none";

/** Bloat severity tiers of the plan (S0 decision 3). Red wins over yellow. */
export function severity(bloat: BloatRow | undefined): Severity {
  if (bloat === undefined) return "none";
  if (bloat.is_na) return "na";
  const pct = bloat.bloat_pct;
  const bytes = bloat.bloat_bytes;
  if (pct === null || bytes === null) return "na";
  if (pct > 50 && bytes > 10 * 1024 * 1024) return "red";
  if (pct > 30 && bytes > 1024 * 1024) return "yellow";
  return "none";
}

type SortKey =
  | "name"
  | "total_bytes"
  | "n_live_tup"
  | "n_dead_tup"
  | "bloat_pct"
  | "bloat_bytes"
  | "last_av"
  | "seq_scan";

interface Column {
  key: SortKey | "severity";
  label: string;
  numeric: boolean;
  title?: string;
}

const COLUMNS: Column[] = [
  { key: "severity", label: "!", numeric: false },
  { key: "name", label: "Table", numeric: false },
  { key: "total_bytes", label: "Size", numeric: true },
  { key: "n_live_tup", label: "Live", numeric: true },
  { key: "n_dead_tup", label: "Dead", numeric: true },
  {
    key: "bloat_pct",
    label: "Bloat %",
    numeric: true,
    title: NO_ESTIMATE_TITLE,
  },
  {
    key: "bloat_bytes",
    label: "Bloat",
    numeric: true,
    title: NO_ESTIMATE_TITLE,
  },
  { key: "last_av", label: "Last AV", numeric: true },
  { key: "seq_scan", label: "Seq/Idx", numeric: true },
];

function tableBloat(
  schema: SchemaSnapshot,
  table: TableStatRow,
): BloatRow | undefined {
  return schema.table_bloat.find(
    (b) => b.schema === table.schema && b.name === table.name,
  );
}

function indexBloat(schema: SchemaSnapshot, table: TableStatRow): BloatRow[] {
  return schema.index_bloat.filter(
    (b) => b.schema === table.schema && b.table === table.name,
  );
}

function lastAv(table: TableStatRow): number | null {
  return table.last_autovacuum_epoch_secs ?? table.last_vacuum_epoch_secs;
}

/** Case-insensitive substring match over schema name, table name, and the
 * fully-qualified `schema.table` (covers a term that straddles the dot) —
 * mirrors the TUI's `schema_row_matches` (v0.12). `needle` is already
 * lowercased by the caller, same convention as `table.ts::rowMatches`. */
export function schemaRowMatches(table: TableStatRow, needle: string): boolean {
  return (
    table.schema.toLowerCase().includes(needle) ||
    table.name.toLowerCase().includes(needle) ||
    `${table.schema}.${table.name}`.toLowerCase().includes(needle)
  );
}

/** Numeric value each sortable column orders by (missing sorts last). */
function sortValue(
  key: SortKey,
  table: TableStatRow,
  bloat: BloatRow | undefined,
): number | string {
  switch (key) {
    case "name":
      return `${table.schema}.${table.name}`;
    case "bloat_pct":
      return (bloat?.is_na ? null : (bloat?.bloat_pct ?? null)) ?? -1;
    case "bloat_bytes":
      return (bloat?.is_na ? null : (bloat?.bloat_bytes ?? null)) ?? -1;
    case "last_av":
      return lastAv(table) ?? -1;
    default:
      return table[key] ?? -1;
  }
}

export class SchemaLens {
  private sortKey: SortKey = "total_bytes";
  private sortAsc = false;
  private snapshot: SchemaSnapshot | null = null;
  private database = "";
  /** `schema.name` keys of rows whose index-bloat detail is open. */
  private readonly expanded = new Set<string>();
  /** v0.12: case-insensitive substring filter (schema + table name),
   * mirroring the TUI's Schema Lens `/` filter. Empty = no filter. */
  private filter = "";
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly staleness: HTMLElement;
  private readonly warning: HTMLElement;
  private readonly placeholder: HTMLElement;

  // Plain assignment, not TS constructor-parameter-property shorthand: the
  // shorthand form is `SyntaxError`-incompatible with Node's built-in
  // strip-only TS loader (`node --test` imports this module directly, no
  // bundler in between) — same reasoning `index-advisor.ts`/`table.ts`
  // already document by NOT using the shorthand.
  constructor(
    table: HTMLTableElement,
    staleness: HTMLElement,
    warning: HTMLElement,
    placeholder: HTMLElement,
    filterInput?: HTMLInputElement | null,
  ) {
    this.staleness = staleness;
    this.warning = warning;
    this.placeholder = placeholder;
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

  update(schema: SchemaSnapshot | null, database: string): void {
    // A fresh collection did not run this tick when collected_at is equal —
    // skip the re-render so open detail rows / hover states stay put.
    const changed =
      schema?.collected_at_epoch_ms !==
      this.snapshot?.collected_at_epoch_ms;
    this.snapshot = schema;
    this.database = database;
    this.renderStaleness();
    this.renderWarning();
    this.placeholder.hidden = schema !== null;
    if (changed) this.renderBody();
  }

  private setSort(key: SortKey): void {
    if (this.sortKey === key) {
      this.sortAsc = !this.sortAsc;
    } else {
      this.sortKey = key;
      // Numbers want "biggest first" on first click; the name column A→Z.
      this.sortAsc = key === "name";
    }
    this.renderHead();
    this.renderBody();
  }

  private renderHead(): void {
    const tr = document.createElement("tr");
    for (const col of COLUMNS) {
      const th = document.createElement("th");
      th.textContent = col.label;
      if (col.title !== undefined) th.title = col.title;
      if (col.key !== "severity") {
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

  private renderStaleness(): void {
    const s = this.snapshot;
    if (s === null) {
      this.staleness.textContent = "";
      return;
    }
    const ageSecs = Math.max(0, (Date.now() - s.collected_at_epoch_ms) / 1000);
    // Bloat is on-demand (its queries are slow) and can only be triggered
    // from the TUI (`R`); the web shows the last estimate if one exists, or
    // notes that none has been taken.
    const bloatNote =
      s.table_bloat.length === 0 && s.index_bloat.length === 0
        ? "bloat: on-demand (run R in the TUI)"
        : "estimated bloat";
    // v0.12: shown/total once a filter narrows the list — mirrors the
    // activity table's count element, folded into this same staleness line
    // (the Schema tab has no separate count badge next to its heading).
    const countText = this.filter
      ? `${s.tables.filter((t) => schemaRowMatches(t, this.filter)).length}/${s.tables.length} tables`
      : `${s.tables.length} tables`;
    this.staleness.textContent =
      `db: ${this.database} · ${countText} · ` +
      `collected ${humanDuration(ageSecs)} ago · ${bloatNote}`;
  }

  private renderWarning(): void {
    const status = this.snapshot?.status;
    if (typeof status === "object" && "Error" in status) {
      this.warning.textContent = `schema: ${status.Error} — showing last collection`;
      this.warning.hidden = false;
    } else {
      this.warning.hidden = true;
    }
  }

  private sorted(schema: SchemaSnapshot): TableStatRow[] {
    const key = this.sortKey;
    const dir = this.sortAsc ? 1 : -1;
    const visible = this.filter
      ? schema.tables.filter((t) => schemaRowMatches(t, this.filter))
      : schema.tables;
    return [...visible].sort((a, b) => {
      const va = sortValue(key, a, tableBloat(schema, a));
      const vb = sortValue(key, b, tableBloat(schema, b));
      if (typeof va === "number" && typeof vb === "number") {
        return (va - vb) * dir;
      }
      return String(va).localeCompare(String(vb)) * dir;
    });
  }

  private renderBody(): void {
    const schema = this.snapshot;
    if (schema === null) {
      this.tbody.replaceChildren();
      return;
    }
    const now = Date.now() / 1000;
    const rows: HTMLTableRowElement[] = [];
    for (const table of this.sorted(schema)) {
      const rowKey = `${table.schema}.${table.name}`;
      rows.push(this.dataRow(schema, table, rowKey, now));
      if (this.expanded.has(rowKey)) {
        rows.push(this.detailRow(schema, table));
      }
    }
    this.tbody.replaceChildren(...rows);
  }

  private dataRow(
    schema: SchemaSnapshot,
    table: TableStatRow,
    rowKey: string,
    now: number,
  ): HTMLTableRowElement {
    const bloat = tableBloat(schema, table);
    const tier = severity(bloat);
    const tr = document.createElement("tr");
    tr.classList.add("schema-row");
    if (tier === "red") tr.classList.add("bloat-red");
    else if (tier === "yellow") tr.classList.add("bloat-yellow");
    else if (tier === "na") tr.classList.add("bloat-na");

    const marker = tier === "red" ? "!!" : tier === "yellow" ? "!" : "";
    let bloatPct = NO_ESTIMATE;
    let bloatBytes = NO_ESTIMATE;
    if (bloat !== undefined && !bloat.is_na) {
      if (bloat.bloat_pct !== null) bloatPct = `${bloat.bloat_pct.toFixed(1)}%`;
      if (bloat.bloat_bytes !== null) bloatBytes = humanBytes(bloat.bloat_bytes);
    }
    const seqIdx = `${humanCount(table.seq_scan)}/${
      table.idx_scan === null ? "—" : humanCount(table.idx_scan)
    }`;
    const cells: Array<[string, boolean]> = [
      [marker, false],
      [rowKey, false],
      [humanBytes(table.total_bytes), true],
      [humanCount(table.n_live_tup), true],
      [humanCount(table.n_dead_tup), true],
      [bloatPct, true],
      [bloatBytes, true],
      [humanAgo(lastAv(table), now), true],
      [seqIdx, true],
    ];
    for (const [text, numeric] of cells) {
      const td = document.createElement("td");
      td.textContent = text;
      if (numeric) td.classList.add("num");
      if (text === NO_ESTIMATE) td.title = NO_ESTIMATE_TITLE;
      tr.append(td);
    }
    tr.title = "click for index bloat detail";
    tr.addEventListener("click", () => {
      if (this.expanded.has(rowKey)) this.expanded.delete(rowKey);
      else this.expanded.add(rowKey);
      this.renderBody();
    });
    return tr;
  }

  /** Full-width detail row: the table's btree indexes with their bloat. */
  private detailRow(
    schema: SchemaSnapshot,
    table: TableStatRow,
  ): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("schema-detail");
    const td = document.createElement("td");
    td.colSpan = COLUMNS.length;
    const indexes = indexBloat(schema, table);
    if (indexes.length === 0) {
      td.textContent = "no index bloat estimates for this table";
    } else {
      const lines = indexes.map((idx) => {
        const pct =
          idx.is_na || idx.bloat_pct === null
            ? NO_ESTIMATE
            : `${idx.bloat_pct.toFixed(1)}%`;
        const bytes =
          idx.is_na || idx.bloat_bytes === null
            ? NO_ESTIMATE
            : humanBytes(idx.bloat_bytes);
        const ff = idx.fillfactor === null ? "—" : String(idx.fillfactor);
        return `${idx.name} · ${humanBytes(idx.real_bytes)} real · ${pct} bloat (${bytes}) · fillfactor ${ff}`;
      });
      td.textContent = lines.join("\n");
      if (indexes.some((idx) => idx.is_na)) td.title = NO_ESTIMATE_TITLE;
    }
    tr.append(td);
    return tr;
  }
}
