// Index advisor (F3): the Schema Lens's "Indexes" sub-view, mirroring the
// TUI's schema_lens.rs conventions:
// - fixed severity-then-size row order (no per-column sort in v1, matching
//   the TUI's SchemaView::Indexes — see crates/pg_lens_tui/src/app.rs);
// - flag column: "INVALID" red / "UNUSED" red / "DUP" yellow / "prefix"
//   dim-yellow / "" ok;
// - staleness line names the stats-reset age, not just collection age — an
//   `idx_scan = 0` claim means nothing right after a reset (PRD pillar 6);
// - clicking a row toggles a detail row: the verbatim indexdef, the finding
//   spelled out, and (for a duplicate) the partner's name as evidence.

import type { IndexFinding, IndexRow, SchemaSnapshot } from "./types";
import { humanAgo, humanBytes, humanCount, humanDuration } from "./format.ts";

export type Severity = "invalid" | "unused" | "dup" | "prefix" | "none";

/** Ranked the same way as the TUI's `index_finding_rank`: lower = worse. */
export function severity(finding: IndexFinding): Severity {
  if (finding === "Invalid") return "invalid";
  if (finding === "Unused") return "unused";
  if (typeof finding === "object" && "DuplicateExact" in finding) return "dup";
  if (typeof finding === "object" && "DuplicatePrefix" in finding) return "prefix";
  return "none";
}

export function severityRank(finding: IndexFinding): number {
  switch (severity(finding)) {
    case "invalid":
      return 0;
    case "unused":
      return 1;
    case "dup":
      return 2;
    case "prefix":
      return 3;
    case "none":
      return 4;
  }
}

export function marker(finding: IndexFinding): string {
  switch (severity(finding)) {
    case "invalid":
      return "INVALID";
    case "unused":
      return "UNUSED";
    case "dup":
      return "DUP";
    case "prefix":
      return "prefix";
    case "none":
      return "";
  }
}

export function partnerOf(finding: IndexFinding): string | null {
  if (typeof finding === "object" && "DuplicateExact" in finding) {
    return finding.DuplicateExact.partner;
  }
  if (typeof finding === "object" && "DuplicatePrefix" in finding) {
    return finding.DuplicatePrefix.partner;
  }
  return null;
}

const COLUMNS = ["Index", "Table", "Size", "Scans", "Tup Read", "Flag"];

export class IndexAdvisor {
  private snapshot: SchemaSnapshot | null = null;
  private database = "";
  /** `schema.table.name` keys of rows whose detail is open. */
  private readonly expanded = new Set<string>();
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly staleness: HTMLElement;
  private readonly warning: HTMLElement;
  private readonly placeholder: HTMLElement;

  constructor(
    table: HTMLTableElement,
    staleness: HTMLElement,
    warning: HTMLElement,
    placeholder: HTMLElement,
  ) {
    this.staleness = staleness;
    this.warning = warning;
    this.placeholder = placeholder;
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.renderHead();
    // Local 1s tick so "collected Xs ago" advances between SSE frames.
    setInterval(() => this.renderStaleness(), 1000);
  }

  update(schema: SchemaSnapshot | null, database: string): void {
    // A fresh collection did not run this tick when collected_at is equal —
    // skip the re-render so open detail rows stay put.
    const changed =
      schema?.collected_at_epoch_ms !== this.snapshot?.collected_at_epoch_ms;
    this.snapshot = schema;
    this.database = database;
    this.renderStaleness();
    this.renderWarning();
    this.placeholder.hidden = schema !== null;
    if (changed) this.renderBody();
  }

  private renderHead(): void {
    const tr = document.createElement("tr");
    for (const label of COLUMNS) {
      const th = document.createElement("th");
      th.textContent = label;
      if (label !== "Index" && label !== "Table" && label !== "Flag") {
        th.classList.add("num");
      }
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
    const now = Date.now() / 1000;
    const resetAge =
      s.stats_reset_epoch_secs === null
        ? "stats reset: unknown"
        : `stats reset ${humanAgo(s.stats_reset_epoch_secs, now)}`;
    this.staleness.textContent =
      `db: ${this.database} · ${s.indexes.length} indexes · ` +
      `collected ${humanDuration(ageSecs)} ago · ${resetAge} · ` +
      `signal, not verdict — verify against the workload`;
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

  /** Fixed severity-then-size order — no per-column sort in v1, matching
   * the TUI's `SchemaView::Indexes`. Ties break by schema/table/name. */
  private sorted(schema: SchemaSnapshot): IndexRow[] {
    return [...schema.indexes].sort((a, b) => {
      const rankDiff = severityRank(a.finding) - severityRank(b.finding);
      if (rankDiff !== 0) return rankDiff;
      const sizeDiff = b.index_bytes - a.index_bytes;
      if (sizeDiff !== 0) return sizeDiff;
      const ka = `${a.schema}.${a.table}.${a.name}`;
      const kb = `${b.schema}.${b.table}.${b.name}`;
      return ka.localeCompare(kb);
    });
  }

  private renderBody(): void {
    const schema = this.snapshot;
    if (schema === null) {
      this.tbody.replaceChildren();
      return;
    }
    const rows: HTMLTableRowElement[] = [];
    for (const idx of this.sorted(schema)) {
      const rowKey = `${idx.schema}.${idx.table}.${idx.name}`;
      rows.push(this.dataRow(idx, rowKey));
      if (this.expanded.has(rowKey)) rows.push(this.detailRow(schema, idx));
    }
    this.tbody.replaceChildren(...rows);
  }

  private dataRow(idx: IndexRow, rowKey: string): HTMLTableRowElement {
    const tier = severity(idx.finding);
    const tr = document.createElement("tr");
    tr.classList.add("index-row");
    if (tier === "invalid") tr.classList.add("idx-invalid");
    else if (tier === "unused") tr.classList.add("idx-unused");
    else if (tier === "dup") tr.classList.add("idx-dup");
    else if (tier === "prefix") tr.classList.add("idx-prefix");

    const cells: Array<[string, boolean]> = [
      [idx.name, false],
      [idx.table, false],
      [humanBytes(idx.index_bytes), true],
      [humanCount(idx.idx_scan), true],
      [humanCount(idx.idx_tup_read), true],
      [marker(idx.finding), false],
    ];
    for (const [text, numeric] of cells) {
      const td = document.createElement("td");
      td.textContent = text;
      if (numeric) td.classList.add("num");
      tr.append(td);
    }
    tr.title = "click for the full definition + finding";
    tr.addEventListener("click", () => {
      if (this.expanded.has(rowKey)) this.expanded.delete(rowKey);
      else this.expanded.add(rowKey);
      this.renderBody();
    });
    return tr;
  }

  /** Full-width detail row: the verbatim indexdef, flags, and the finding
   * spelled out with its duplicate partner (if any) — evidence, not a bare
   * label (PRD pillar 6: signal, not verdict). */
  private detailRow(
    schema: SchemaSnapshot,
    idx: IndexRow,
  ): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("index-detail");
    const td = document.createElement("td");
    td.colSpan = COLUMNS.length;
    const flags: string[] = [];
    if (idx.is_primary) flags.push("primary key");
    else if (idx.is_unique) flags.push("unique");
    if (idx.is_exclusion) flags.push("exclusion");
    if (idx.is_constraint && !idx.is_primary) flags.push("constraint-backed");
    const flagsText = flags.length > 0 ? flags.join(" · ") : "plain (non-unique, no constraint)";

    const findingText = findingDescription(idx.finding);
    const now = Date.now() / 1000;
    const lines = [
      idx.indexdef,
      `flags: ${flagsText}`,
      `scans: ${humanCount(idx.idx_scan)} · tuples read ${humanCount(idx.idx_tup_read)} · tuples fetched ${humanCount(idx.idx_tup_fetch)}`,
      `stats freshness: counters ${humanAgo(schema.stats_reset_epoch_secs, now)}`,
      `finding: ${findingText}`,
    ];
    td.textContent = lines.join("\n");
    tr.append(td);
    return tr;
  }
}

export function findingDescription(finding: IndexFinding): string {
  const partner = partnerOf(finding);
  switch (severity(finding)) {
    case "invalid":
      return "INVALID — indisvalid/indisready is false: a CREATE INDEX CONCURRENTLY likely failed or was cancelled; dead weight (never served to the planner, still costs every write), can safely be dropped and rebuilt";
    case "unused":
      return "UNUSED — zero scans since the last stats reset; serves no constraint";
    case "dup":
      return `DUP — exact duplicate of '${partner}' (same columns, opclasses, predicate and uniqueness)`;
    case "prefix":
      return `prefix — this index's columns are a strict prefix of '${partner}''s; the wider index can likely serve both`;
    case "none":
      return "no finding — in use, or uniquely useful";
  }
}
