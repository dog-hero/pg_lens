// Vacuum / wraparound section (F2), rendered under the Schema table —
// mirrors the TUI's schema_lens.rs "Vacuum / wraparound" panel: cluster-wide
// XID wraparound headline, the worst per-table ages with their dead-tuple
// ratio, and any in-flight pg_stat_progress_vacuum row (a calm "no vacuum
// running" in the common empty case).

import type { SchemaSnapshot, VacuumProgressRow } from "./types";
import { humanCount } from "./format";
import { ageSeverity } from "./vacuum";

/** Capped like the TUI's VACUUM_TABLE_ROWS, so the panel stays compact. */
const WORST_TABLES_SHOWN = 3;

function deadPct(dead: number, live: number): number {
  const total = dead + live;
  return total > 0 ? (dead / total) * 100 : 0;
}

export class VacuumPanel {
  constructor(
    private readonly cluster: HTMLElement,
    private readonly tables: HTMLUListElement,
    private readonly progress: HTMLElement,
  ) {}

  update(
    schema: SchemaSnapshot | null,
    vacuumProgress: VacuumProgressRow[] | null,
  ): void {
    this.renderCluster(schema);
    this.renderTables(schema);
    this.renderProgress(vacuumProgress);
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
    const items = rows.slice(0, WORST_TABLES_SHOWN).map((t) => {
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
}
