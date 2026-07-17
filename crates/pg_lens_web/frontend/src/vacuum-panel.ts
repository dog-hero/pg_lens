// Vacuum / wraparound section (F2), rendered under the Schema table —
// mirrors the TUI's schema_lens.rs "Vacuum / wraparound" panel: cluster-wide
// XID wraparound headline, the worst per-table ages with their dead-tuple
// ratio, and any in-flight pg_stat_progress_vacuum row (a calm "no vacuum
// running" in the common empty case).
//
// U3: unlike the TUI (which needed a `v` sub-view to make room), the web
// panel already sits below the full Schema table with plenty of vertical
// room, so it renders the COMPLETE worst-tables list (all
// `VACUUM_TABLES_LIMIT` rows the query ships) — no toggle needed here.

import type { PreparedXactRow, SchemaSnapshot, VacuumProgressRow } from "./types";
import { humanCount, humanDuration } from "./format";
import { ageSeverity } from "./vacuum";
import { preparedXactSeverity } from "./prepared_xacts";

function deadPct(dead: number, live: number): number {
  const total = dead + live;
  return total > 0 ? (dead / total) * 100 : 0;
}

export class VacuumPanel {
  constructor(
    private readonly cluster: HTMLElement,
    private readonly tables: HTMLUListElement,
    private readonly progress: HTMLElement,
    private readonly preparedXacts: HTMLUListElement,
  ) {}

  update(
    schema: SchemaSnapshot | null,
    vacuumProgress: VacuumProgressRow[] | null,
    preparedXacts: PreparedXactRow[] | null,
  ): void {
    this.renderCluster(schema);
    this.renderTables(schema);
    this.renderProgress(vacuumProgress);
    this.renderPreparedXacts(preparedXacts);
  }

  private renderCluster(schema: SchemaSnapshot | null): void {
    const age = schema?.vacuum_cluster_age ?? null;
    if (age === null) {
      this.cluster.textContent = "wraparound: collecting…";
      this.cluster.className = "vacuum-cluster";
      return;
    }
    const sev = ageSeverity(age.max_age_xids);
    this.cluster.textContent =
      `wraparound: ${humanCount(age.max_age_xids)} xids ` +
      `(worst db: ${age.worst_database})`;
    this.cluster.className = sev ? `vacuum-cluster ${sev}` : "vacuum-cluster";
  }

  private renderTables(schema: SchemaSnapshot | null): void {
    const rows = schema?.vacuum_tables ?? [];
    if (rows.length === 0) {
      const li = document.createElement("li");
      li.className = "vacuum-empty";
      li.textContent = "(no per-table XID ages collected yet)";
      this.tables.replaceChildren(li);
      return;
    }
    const items = rows.map((t) => {
      const sev = ageSeverity(t.age_xids);
      const li = document.createElement("li");
      li.className = sev ? `vacuum-table ${sev}` : "vacuum-table";
      li.textContent =
        `${t.schema}.${t.name} — ${humanCount(t.age_xids)} xids · ` +
        `${deadPct(t.n_dead_tup, t.n_live_tup).toFixed(1)}% dead`;
      return li;
    });
    this.tables.replaceChildren(...items);
  }

  private renderProgress(rows: VacuumProgressRow[] | null): void {
    if (rows === null) {
      this.progress.textContent = "vacuum progress: unavailable";
      this.progress.className = "vacuum-progress dim";
      return;
    }
    if (rows.length === 0) {
      this.progress.textContent = "no vacuum running";
      this.progress.className = "vacuum-progress dim";
      return;
    }
    const row = rows[0];
    if (row === undefined) {
      this.progress.textContent = "no vacuum running";
      this.progress.className = "vacuum-progress dim";
      return;
    }
    const pct =
      row.heap_blks_total > 0
        ? (100 * row.heap_blks_scanned) / row.heap_blks_total
        : 0;
    this.progress.textContent =
      `vacuuming ${row.relation} — ${row.phase} (${pct.toFixed(0)}%)`;
    this.progress.className = "vacuum-progress";
  }

  /**
   * v0.9: orphaned two-phase-commit watch (`pg_prepared_xacts`) — the
   * classic silent incident, rendered right below the vacuum progress it
   * blocks. `null` (best-effort collection failed this tick) renders a dim
   * "unavailable" line; an empty array (the overwhelmingly common case)
   * renders a dim "none" line; otherwise one severity-colored row per
   * dangling prepared transaction (gid, owner, database, age).
   */
  private renderPreparedXacts(rows: PreparedXactRow[] | null): void {
    if (rows === null) {
      const li = document.createElement("li");
      li.className = "vacuum-empty";
      li.textContent = "prepared transactions: unavailable";
      this.preparedXacts.replaceChildren(li);
      return;
    }
    if (rows.length === 0) {
      const li = document.createElement("li");
      li.className = "vacuum-empty";
      li.textContent = "no orphaned prepared transactions";
      this.preparedXacts.replaceChildren(li);
      return;
    }
    const items = rows.map((row) => {
      const sev = preparedXactSeverity(row.age_seconds);
      const li = document.createElement("li");
      li.className = sev ? `vacuum-table ${sev}` : "vacuum-table";
      li.textContent =
        `${row.gid} — owner ${row.owner} · db ${row.database} · ` +
        `age ${humanDuration(row.age_seconds)}`;
      return li;
    });
    this.preparedXacts.replaceChildren(...items);
  }
}
