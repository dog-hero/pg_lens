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

import type {
  BloatRow,
  SchemaSnapshot,
  TableDetail,
  TableDetailColumn,
  TableStatRow,
} from "./types";
import {
  humanAgo,
  humanBytes,
  humanBytesSigned,
  humanCount,
  humanDuration,
} from "./format.ts";

const NO_ESTIMATE = "~?";
const NO_ESTIMATE_TITLE = "estimated (needs fresh ANALYZE)";
const NO_GROWTH = "—";

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

/** v0.14 growth severity — mirrors `pg_lens_core::schema_growth::{severity,
 * WARN_GROWTH_PCT, BAD_GROWTH_PCT, SEVERITY_MIN_TABLE_BYTES}` exactly.
 * Never colors a table under the absolute size floor, no matter the
 * percentage — a 2 KB scratch table doubling is not an incident. */
export const WARN_GROWTH_PCT = 10;
export const BAD_GROWTH_PCT = 25;
export const SEVERITY_MIN_TABLE_BYTES = 10 * 1024 * 1024;

export function growthSeverity(
  totalBytes: number,
  growthPct: number | null,
): "red" | "yellow" | "none" {
  if (totalBytes < SEVERITY_MIN_TABLE_BYTES || growthPct === null)
    return "none";
  const pct = Math.abs(growthPct);
  if (pct > BAD_GROWTH_PCT) return "red";
  if (pct > WARN_GROWTH_PCT) return "yellow";
  return "none";
}

/** v0.15: the honest table-count clause, composing the v0.12 filter's
 * `shown/fetched` with the true (uncapped) total — mirrors the TUI's
 * `ui/schema_lens.rs::table_count_text`. `fetched` is `tables.length` (the
 * possibly-truncated list this snapshot carries); `total` is
 * `tables_total` (`null` only before the first successful collection).
 * `shown` is the post-filter row count (`=== fetched` when unfiltered). */
export function tableCountText(
  shown: number,
  fetched: number,
  total: number | null,
): string {
  const filtered = shown !== fetched;
  const truncated = total !== null && total > fetched;
  const base = filtered ? `${shown}/${fetched}` : `${fetched}`;
  return truncated
    ? `${base} of ${total} tables — raise schema_table_limit or filter`
    : `${base} tables`;
}

/** v0.15's partition-collapsing hint, appended to the staleness line —
 * mirrors the TUI's `partitions_hint` exactly (same terse wording, same
 * "no hint when there are no partitioned tables at all" rule). */
export function partitionsHint(
  tables: TableStatRow[],
  showPartitions: boolean,
): string {
  const leafCount = tables.filter((t) => t.is_partition).length;
  if (leafCount === 0) return "";
  return showPartitions ? " · parts shown" : ` · +${leafCount} parts`;
}

type SortKey =
  | "name"
  | "total_bytes"
  | "growth_1h_bytes"
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
  { key: "growth_1h_bytes", label: "Δ1h", numeric: true },
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
  /** `schema.name` keys of rows whose index-bloat/structure detail is open. */
  private readonly expanded = new Set<string>();
  /** v0.15: on-demand `\d`-style table detail (columns/constraints/indexes)
   * of whichever table was last requested — `null` before any request this
   * session. Matched against `TableDetail.oid` before rendering, since the
   * response can lag a tick or two behind the click (see `detailRow`). The
   * poller only caches ONE table's detail at a time, so unlike a normal
   * "fetch once" cache, this is re-requested every time a row is (re-)
   * expanded — a previously-fetched table's detail can have been evicted by
   * a different table's request meanwhile. */
  private tableDetail: TableDetail | null = null;
  /** v0.12: case-insensitive substring filter (schema + table name),
   * mirroring the TUI's Schema Lens `/` filter. Empty = no filter. */
  private filter = "";
  /** v0.15's partition collapsing: `false` (default) hides
   * `TableStatRow.is_partition` rows in favor of their aggregated parent
   * row; the "show partitions" checkbox flips it. Mirrors the TUI's
   * `App.schema_show_partitions` exactly. */
  private showPartitions = false;
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly staleness: HTMLElement;
  private readonly warning: HTMLElement;
  private readonly placeholder: HTMLElement;
  private readonly onDetailRequest: ((oid: number, schema: string, name: string) => void) | null;
  private readonly onJumpToIndexes: ((tableName: string) => void) | null;
  private readonly onInspectTable: ((table: TableStatRow, schema: SchemaSnapshot, detail?: TableDetail | null) => void) | null;

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
    onDetailRequest?: (oid: number, schema: string, name: string) => void,
    partitionsToggle?: HTMLInputElement | null,
    onJumpToIndexes?: (tableName: string) => void,
    onInspectTable?: (table: TableStatRow, schema: SchemaSnapshot, detail?: TableDetail | null) => void,
  ) {
    this.staleness = staleness;
    this.warning = warning;
    this.placeholder = placeholder;
    this.onDetailRequest = onDetailRequest ?? null;
    this.onJumpToIndexes = onJumpToIndexes ?? null;
    this.onInspectTable = onInspectTable ?? null;
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
    if (partitionsToggle) {
      partitionsToggle.checked = this.showPartitions;
      partitionsToggle.addEventListener("change", () => {
        this.showPartitions = partitionsToggle.checked;
        this.renderStaleness();
        this.renderBody();
      });
    }
  }

  update(
    schema: SchemaSnapshot | null,
    database: string,
    tableDetail: TableDetail | null = null,
  ): void {
    // A fresh collection did not run this tick when collected_at is equal —
    // skip the re-render so open detail rows / hover states stay put.
    const changed =
      schema?.collected_at_epoch_ms !== this.snapshot?.collected_at_epoch_ms;
    // A structure fetch landed (or was superseded) since the last render.
    const detailChanged = tableDetail !== this.tableDetail;
    this.snapshot = schema;
    this.database = database;
    this.tableDetail = tableDetail;
    this.renderStaleness();
    this.renderWarning();
    this.placeholder.hidden = schema !== null;
    if (changed || detailChanged) this.renderBody();
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
    // v0.12: shown/fetched once a filter narrows the list, folded into this
    // same staleness line (the Schema tab has no separate count badge next
    // to its heading). v0.15: composed with the true (uncapped) total, so a
    // truncated `tables` list never reads as complete.
    const shown = this.filter
      ? s.tables.filter((t) => schemaRowMatches(t, this.filter)).length
      : s.tables.length;
    const countText = tableCountText(shown, s.tables.length, s.tables_total);
    const partitionsNote = partitionsHint(s.tables, this.showPartitions);
    this.staleness.textContent =
      `db: ${this.database} · ${countText}${partitionsNote} · ` +
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
    // v0.15's partition collapsing composes with the `/` filter: leaf
    // partitions are dropped from the DISPLAYED rows unless the toggle is
    // on, but the filter itself still matches every row (parents AND,
    // once expanded, leaves) — same rule as the TUI's `resort_schema`.
    const visible = schema.tables.filter(
      (t) =>
        (this.showPartitions || !t.is_partition) &&
        (!this.filter || schemaRowMatches(t, this.filter)),
    );
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
    const growthText =
      table.growth_1h_bytes === null
        ? NO_GROWTH
        : humanBytesSigned(table.growth_1h_bytes);
    const growthTier = growthSeverity(table.total_bytes, table.growth_1h_pct);
    // v0.15's partition collapsing: a parent gets a trailing `[parts: N]`
    // marker; a leaf (only reachable when expanded) gets a "↳ " prefix so
    // it still reads as subordinate to its parent out of drill-down
    // context. Mirrors the TUI's `name_text`/`name_style` exactly.
    const nameText =
      table.partition_count !== null
        ? `${rowKey} [parts: ${table.partition_count}]`
        : table.is_partition
          ? `↳ ${rowKey}`
          : rowKey;
    if (table.is_partition) tr.classList.add("partition-leaf");
    const cells: Array<[string, boolean, string | undefined]> = [
      [marker, false, undefined],
      [nameText, false, undefined],
      [humanBytes(table.total_bytes), true, undefined],
      [growthText, true, growthTier === "none" ? undefined : `growth-${growthTier}`],
      [humanCount(table.n_live_tup), true, undefined],
      [humanCount(table.n_dead_tup), true, undefined],
      [bloatPct, true, undefined],
      [bloatBytes, true, undefined],
      [humanAgo(lastAv(table), now), true, undefined],
      [seqIdx, true, undefined],
    ];
    for (const [text, numeric, cls] of cells) {
      const td = document.createElement("td");
      td.textContent = text;
      if (numeric) td.classList.add("num");
      if (cls !== undefined) td.classList.add(cls);
      if (text === NO_ESTIMATE) td.title = NO_ESTIMATE_TITLE;
      tr.append(td);
    }
    // v0.15's per-table lock indicator: a compact `L:N`/`L:N!` badge
    // trailing the table name, mirroring the TUI's inline marker exactly
    // (a dedicated column would need a header/width change for no real
    // gain — see `ui/schema_lens.rs::draw_table`'s doc comment for the same
    // reasoning). Red bold when any waiter, dim when granted-only, absent
    // entirely when `lock_count` is `null` (no lock data this tick, or
    // genuinely zero locks — see `TableStatRow.lock_count`'s doc comment).
    if (table.lock_count !== null) {
      const waiting = table.lock_waiters ?? 0;
      const badge = document.createElement("span");
      badge.classList.add("lock-badge", waiting > 0 ? "lock-badge-red" : "lock-badge-dim");
      badge.textContent = waiting > 0 ? ` L:${table.lock_count}!` : ` L:${table.lock_count}`;
      tr.children[1]?.append(badge);
    }
    tr.title = "click for structure + index bloat detail";
    tr.addEventListener("click", () => {
      this.onInspectTable?.(table, schema, this.tableDetail);
      if (this.expanded.has(rowKey)) {
        this.expanded.delete(rowKey);
      } else {
        this.expanded.add(rowKey);
        // v0.15: fires the on-demand `\d` fetch every time a row is
        // (re-)expanded — the poller caches only ONE table's detail at a
        // time, so a stale cached detail from a different table must always
        // be superseded, not assumed still fresh.
        this.onDetailRequest?.(table.oid, table.schema, table.name);
      }
      this.renderBody();
    });
    return tr;
  }

  /** Full-width detail row: the table's `\d`-style structure (columns,
   * constraints, referenced-by, index definitions) plus its btree index
   * bloat — the two on-demand/slow-cadence pieces the Schema Lens carries
   * for one table, stacked in a single expanded row. */
  private detailRow(
    schema: SchemaSnapshot,
    table: TableStatRow,
  ): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("schema-detail");
    const td = document.createElement("td");
    td.colSpan = COLUMNS.length;
    const children: HTMLElement[] = [];
    // v0.15's partition collapsing: a parent's drill-down section — every
    // leaf grouped by `parent_oid`, sourced entirely from the already-
    // collected `schema.tables` (no extra fetch, unlike the structure
    // section below). Only rendered for a synthesized parent row.
    if (table.partition_count !== null) {
      children.push(this.partitionsSection(schema, table));
    }
    children.push(
      locksSection(table),
      this.storageAndCacheSection(table),
      this.structureSection(table),
      this.bloatSection(schema, table),
    );
    td.append(...children);
    tr.append(td);
    return tr;
  }

  /** Storage breakdown (Heap, TOAST, Indexes, Total) and buffer cache hit
   * ratios (Heap, Index, TOAST) for this table (v0.19). */
  private storageAndCacheSection(table: TableStatRow): HTMLElement {
    const section = document.createElement("div");
    section.classList.add("schema-structure");
    const heading = document.createElement("p");
    heading.classList.add("schema-structure-heading");
    heading.textContent = "storage & buffer cache";
    section.append(heading);
    section.append(preLines(storageAndCacheLines(table)));
    if (this.onJumpToIndexes !== null) {
      const jumpWrap = document.createElement("div");
      jumpWrap.style.marginTop = "6px";
      const btn = document.createElement("button");
      btn.classList.add("ghost-btn", "jump-btn");
      btn.type = "button";
      btn.textContent = `View Indexes for ${table.name} (i) ↗`;
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        this.onJumpToIndexes?.(table.name);
      });
      jumpWrap.append(btn);
      section.append(jumpWrap);
    }
    return section;
  }

  /** v0.15's drill-down: lists every leaf partition of `parent` (matched by
   * `parent_oid`), sorted by name — name, size, live/dead tuples. Mirrors
   * the TUI's `draw_partitions_section` exactly. */
  private partitionsSection(
    schema: SchemaSnapshot,
    parent: TableStatRow,
  ): HTMLElement {
    const section = document.createElement("div");
    section.classList.add("schema-structure");
    const heading = document.createElement("p");
    heading.classList.add("schema-structure-heading");
    heading.textContent = `partitions (${parent.partition_count ?? 0})`;
    section.append(heading);
    const leaves = schema.tables
      .filter((t) => t.parent_oid === parent.oid)
      .sort((a, b) => a.name.localeCompare(b.name));
    if (leaves.length === 0) {
      section.append(dimLine("(no leaves in the current collection)"));
    } else {
      const lines = leaves.map(
        (leaf) =>
          `${leaf.schema}.${leaf.name} · ${humanBytes(leaf.total_bytes)} · ` +
          `live ${humanCount(leaf.n_live_tup)} · dead ${humanCount(leaf.n_dead_tup)}`,
      );
      section.append(preLines(lines));
    }
    return section;
  }

  /** The `\d`-style structure block: "loading…" while the request is in
   * flight / hasn't landed yet, an inline error on a best-effort failure,
   * or columns/constraints/referenced-by/indexes on success. Matched
   * against the table's oid — a detail for a differently expanded table
   * (still catching up after a fresh request) must never render under the
   * wrong table's name. */
  private structureSection(table: TableStatRow): HTMLElement {
    const section = document.createElement("div");
    section.classList.add("schema-structure");
    const detail =
      this.tableDetail !== null && this.tableDetail.oid === table.oid
        ? this.tableDetail
        : null;
    if (detail === null) {
      const p = document.createElement("p");
      p.classList.add("dim");
      p.textContent = "loading structure…";
      section.append(p);
      return section;
    }
    if (detail.error !== null) {
      const p = document.createElement("p");
      p.classList.add("warn");
      p.textContent = structureErrorLine(detail.error);
      section.append(p);
      return section;
    }

    const columnsHeading = document.createElement("p");
    columnsHeading.classList.add("schema-structure-heading");
    columnsHeading.textContent = "columns";
    section.append(columnsHeading);
    const columnLines = columnDetailLines(detail);
    if (columnLines.length === 0) {
      section.append(dimLine("(no columns)"));
    } else {
      section.append(preLines(columnLines));
    }

    const constraintsHeading = document.createElement("p");
    constraintsHeading.classList.add("schema-structure-heading");
    constraintsHeading.textContent = "constraints";
    section.append(constraintsHeading);
    const ownLines = ownConstraintLines(detail);
    if (ownLines.length === 0) {
      section.append(dimLine("(no constraints)"));
    } else {
      section.append(preLines(ownLines));
    }
    const referencingLines = referencingConstraintLines(detail);
    if (referencingLines.length > 0) {
      const refHeading = document.createElement("p");
      refHeading.classList.add("schema-structure-heading");
      refHeading.textContent = "referenced by";
      section.append(refHeading);
      section.append(preLines(referencingLines));
    }

    const indexesHeading = document.createElement("p");
    indexesHeading.classList.add("schema-structure-heading");
    indexesHeading.textContent = "indexes";
    section.append(indexesHeading);
    const indexLines = indexDetailLines(detail);
    if (indexLines.length === 0) {
      section.append(dimLine("(no indexes)"));
    } else {
      section.append(preLines(indexLines));
    }
    return section;
  }

  /** The pre-existing index-bloat block (unchanged behavior, just factored
   * out so `detailRow` can stack it under the new structure section). */
  private bloatSection(schema: SchemaSnapshot, table: TableStatRow): HTMLElement {
    const wrap = document.createElement("div");
    wrap.classList.add("schema-bloat-section");
    const heading = document.createElement("p");
    heading.classList.add("schema-structure-heading");
    heading.textContent = "index bloat";
    wrap.append(heading);
    const indexes = indexBloat(schema, table);
    if (indexes.length === 0) {
      wrap.append(dimLine("no index bloat estimates for this table"));
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
      const pre = preLines(lines);
      if (indexes.some((idx) => idx.is_na)) pre.title = NO_ESTIMATE_TITLE;
      wrap.append(pre);
    }
    return wrap;
  }
}

/** v0.15's per-table lock indicator, spelled out in the detail row (the
 * data row only has room for the compact `L:N`/`L:N!` badge). Mirrors the
 * TUI's `locks_detail_line` exactly: red bold when any waiter, dim when
 * granted-only, dim "no lock data" when this tick's best-effort collection
 * found nothing for this table. */
function locksSection(table: TableStatRow): HTMLElement {
  const section = document.createElement("div");
  section.classList.add("schema-structure");
  const heading = document.createElement("p");
  heading.classList.add("schema-structure-heading");
  heading.textContent = "locks";
  section.append(heading);
  const p = document.createElement("p");
  if (table.lock_count === null) {
    p.classList.add("dim");
    p.textContent = "none this tick";
  } else {
    const waiting = table.lock_waiters ?? 0;
    if (waiting > 0) {
      p.classList.add("bad");
      p.textContent = `${table.lock_count} held, ${waiting} waiting — blocked right now`;
    } else {
      p.classList.add("dim");
      p.textContent = `${table.lock_count} held (granted)`;
    }
  }
  section.append(p);
  return section;
}

/** v0.15: the "structure unavailable" line — a best-effort failure reason
 * (table dropped mid-request, missing privilege) rendered inline, never a
 * silently vanished section. */
export function structureErrorLine(reason: string): string {
  return `structure unavailable: ${reason}`;
}

/** v0.15: the trailing `" default ..."` / `" generated ..."` clause of a
 * column line — leading space included, empty string when the column has
 * neither a default nor an identity/generation marker. Mirrors the TUI's
 * `column_default_text` exactly. Three cases, in priority order (a column
 * is never more than one of these):
 *   * `identity` set — `" generated always as identity"` / `" generated by
 *     default as identity"` (no `pg_attrdef` row exists for these, so
 *     `default` is always `null` alongside it);
 *   * `generated_stored` — `" generated always as (<expr>) stored"`,
 *     `default` holding the STORED generation expression;
 *   * a plain `default` — `" default <expr>"`.
 */
export function columnDefaultText(col: TableDetailColumn): string {
  if (col.identity !== null) return ` ${col.identity}`;
  if (col.default === null) return "";
  return col.generated_stored
    ? ` generated always as (${col.default}) stored`
    : ` default ${col.default}`;
}

/** v0.15: one line per column — `name type [not ]null[ default/identity/
 * generated clause]` — pg_lens's own scannable rendering of `\d`'s column
 * block, not a psql clone. Pure/DOM-free so it is directly unit-testable. */
export function columnDetailLines(detail: TableDetail): string[] {
  return detail.columns.map((c) => {
    const nullText = c.not_null ? "not null" : "null";
    return `${c.name} ${c.data_type} ${nullText}${columnDefaultText(c)}`;
  });
}

/** v0.15: the table's OWN constraints (`referencing_table === null`) as
 * `KIND name: definition` lines. */
export function ownConstraintLines(detail: TableDetail): string[] {
  return detail.constraints
    .filter((c) => c.referencing_table === null)
    .map((c) => `${c.kind} ${c.name}: ${c.definition}`);
}

/** v0.15: foreign keys on OTHER tables pointing back at this one
 * (`referencing_table !== null`, "referenced by") as
 * `other_table.name: definition` lines. */
export function referencingConstraintLines(detail: TableDetail): string[] {
  return detail.constraints
    .filter((c) => c.referencing_table !== null)
    .map((c) => `${c.referencing_table}.${c.name}: ${c.definition}`);
}

/** v0.15: one `pg_get_indexdef` verbatim line per index. */
export function indexDetailLines(detail: TableDetail): string[] {
  return detail.indexes.map((idx) => idx.definition);
}

/** A dim, single-line `<p>` — the calm "nothing here" shape used across the
 * detail row's sections. */
function dimLine(text: string): HTMLElement {
  const p = document.createElement("p");
  p.classList.add("dim");
  p.textContent = text;
  return p;
}

/** A `<pre>` block joining `lines` with newlines — same rendering shape the
 * pre-existing index-bloat block used (`textContent` with `\n`), factored
 * out so the new structure sections reuse it too. */
function preLines(lines: string[]): HTMLPreElement {
  const pre = document.createElement("pre");
  pre.textContent = lines.join("\n");
  return pre;
}

/** Storage and cache hit lines for table detail (v0.19). */
export function storageAndCacheLines(table: TableStatRow): string[] {
  const heap = table.heap_bytes != null ? humanBytes(table.heap_bytes) : "—";
  const toast = table.toast_bytes != null ? humanBytes(table.toast_bytes) : "—";
  const idx = humanBytes(table.index_bytes);
  const total = humanBytes(table.total_bytes);

  const heapHit = table.heap_cache_hit_pct != null ? `${table.heap_cache_hit_pct.toFixed(1)}%` : "—";
  const idxHit = table.idx_cache_hit_pct != null ? `${table.idx_cache_hit_pct.toFixed(1)}%` : "—";
  const toastHit = table.toast_cache_hit_pct != null ? `${table.toast_cache_hit_pct.toFixed(1)}%` : "—";

  return [
    `storage: total ${total} · heap ${heap} · toast ${toast} · indexes ${idx}`,
    `cache hit: heap ${heapHit} · index ${idxHit} · toast ${toastHit}`,
  ];
}
