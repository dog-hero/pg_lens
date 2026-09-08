// Slide-over Inspector Drawer for pg_lens web
// Replaces clumsy inline table expansions with a dedicated, elegant slide-over panel.

import type {
  ActivityRow,
  BloatRow,
  LockRow,
  ReplicationSlotRow,
  SchemaSnapshot,
  StatementRow,
  TableDetail,
  TableStatRow,
} from "./types.ts";
import { humanBytes, humanCount, humanDuration, humanMs } from "./format.ts";
import { renderSqlInto } from "./sql.ts";
import { renderCopyButton } from "./clipboard.ts";
import { blockingChain, renderBlockingChain } from "./blocking.ts";
import type { AdminKind } from "./actions.ts";
import { columnDetailLines, ownConstraintLines, referencingConstraintLines } from "./schema.ts";

export interface DrawerCallbacks {
  onAdmin?: (kind: AdminKind, row: ActivityRow) => void;
  onCopy?: (ok: boolean, chars: number) => void;
}

export class InspectorDrawer {
  private drawerEl: HTMLElement;
  private backdropEl: HTMLElement;
  private titleEl: HTMLElement;
  private badgeEl: HTMLElement;
  private tabsEl: HTMLElement;
  private contentEl: HTMLElement;
  private actionsEl: HTMLElement;
  private closeBtn: HTMLButtonElement;

  private currentTab = "overview";
  private currentTable: { schema: SchemaSnapshot; stat: TableStatRow; detail?: TableDetail | null } | null = null;
  private adminEnabled: () => boolean;
  private callbacks: DrawerCallbacks;

  constructor(
    drawerId = "inspector-drawer",
    backdropId = "drawer-backdrop",
    adminEnabled: () => boolean = () => false,
    callbacks: DrawerCallbacks = {},
  ) {
    this.drawerEl = document.getElementById(drawerId) as HTMLElement;
    this.backdropEl = document.getElementById(backdropId) as HTMLElement;
    this.adminEnabled = adminEnabled;
    this.callbacks = callbacks;

    this.titleEl = this.drawerEl.querySelector(".drawer-title") as HTMLElement;
    this.badgeEl = this.drawerEl.querySelector(".drawer-badge") as HTMLElement;
    this.tabsEl = this.drawerEl.querySelector(".drawer-tabs") as HTMLElement;
    this.contentEl = this.drawerEl.querySelector(".drawer-content") as HTMLElement;
    this.actionsEl = this.drawerEl.querySelector(".drawer-actions") as HTMLElement;
    this.closeBtn = this.drawerEl.querySelector(".drawer-close") as HTMLButtonElement;

    this.closeBtn?.addEventListener("click", () => this.close());
    this.backdropEl?.addEventListener("click", () => this.close());
  }

  isOpen(): boolean {
    return this.drawerEl.classList.contains("open");
  }

  close(): void {
    this.drawerEl.classList.remove("open");
    this.backdropEl.classList.remove("active");
    this.currentTable = null;
  }

  openSession(row: ActivityRow, locks: LockRow[] = []): void {
    this.currentTable = null;
    this.currentTab = "overview";

    this.titleEl.textContent = `Session PID ${row.pid}`;
    this.badgeEl.textContent = row.state;
    this.badgeEl.className = `badge ${row.state === "active" ? "badge-ok" : row.state.includes("idle in transaction") ? "badge-warn" : "badge-info"}`;

    this.renderSessionTabs(row, locks);
    this.renderSessionContent(row, locks);
    this.renderSessionActions(row);

    this.drawerEl.classList.add("open");
    this.backdropEl.classList.add("active");
  }

  openTable(stat: TableStatRow, schema: SchemaSnapshot, detail?: TableDetail | null): void {
    this.currentTable = { schema, stat, detail };
    this.currentTab = "columns";

    this.titleEl.textContent = `${stat.schema}.${stat.name}`;
    this.badgeEl.textContent = humanBytes(stat.total_bytes);
    this.badgeEl.className = "badge badge-info";

    this.renderTableTabs();
    this.renderTableContent();
    this.actionsEl.replaceChildren();

    this.drawerEl.classList.add("open");
    this.backdropEl.classList.add("active");
  }

  openSlot(slot: ReplicationSlotRow): void {
    this.currentTable = null;
    this.currentTab = "slot";

    this.titleEl.textContent = `Slot: ${slot.slot_name}`;
    this.badgeEl.textContent = slot.active ? "ACTIVE" : "INACTIVE";
    this.badgeEl.className = `badge ${slot.active ? "badge-ok" : "badge-warn"}`;

    this.tabsEl.replaceChildren();
    this.renderSlotContent(slot);
    this.actionsEl.replaceChildren();

    this.drawerEl.classList.add("open");
    this.backdropEl.classList.add("active");
  }

  openStatement(stmt: StatementRow): void {
    this.currentTable = null;
    this.currentTab = "query";

    this.titleEl.textContent = stmt.query_id ? `Query ID: ${stmt.query_id}` : "Query Details";
    this.badgeEl.textContent = `${humanCount(stmt.calls)} calls`;
    this.badgeEl.className = "badge badge-info";

    this.tabsEl.replaceChildren();
    this.renderStatementContent(stmt);
    this.actionsEl.replaceChildren();

    this.drawerEl.classList.add("open");
    this.backdropEl.classList.add("active");
  }

  // ── Session rendering ──────────────────────────────────────────────
  private renderSessionTabs(row: ActivityRow, locks: LockRow[]): void {
    this.tabsEl.replaceChildren();
    const isBlocked = locks.some((l) => l.pid === row.pid);

    const tabs = [
      { id: "overview", label: "Overview & SQL" },
      ...(isBlocked ? [{ id: "blocking", label: "Blocking Chain" }] : []),
    ];

    for (const t of tabs) {
      const btn = document.createElement("button");
      btn.className = `drawer-tab ${this.currentTab === t.id ? "active" : ""}`;
      btn.textContent = t.label;
      btn.addEventListener("click", () => {
        this.currentTab = t.id;
        for (const b of this.tabsEl.querySelectorAll(".drawer-tab")) {
          b.classList.remove("active");
        }
        btn.classList.add("active");
        this.renderSessionContent(row, locks);
      });
      this.tabsEl.appendChild(btn);
    }
  }

  private renderSessionContent(row: ActivityRow, locks: LockRow[]): void {
    this.contentEl.replaceChildren();

    if (this.currentTab === "blocking") {
      const wrap = document.createElement("div");
      wrap.className = "drawer-section";
      const h3 = document.createElement("h3");
      h3.textContent = "Wait-for Dependency Chain";
      wrap.appendChild(h3);
      const chain = blockingChain(row.pid, locks);
      if (chain) {
        wrap.appendChild(renderBlockingChain(chain));
      } else {
        const p = document.createElement("p");
        p.className = "placeholder";
        p.textContent = "No blocking wait chain detected for this session.";
        wrap.appendChild(p);
      }
      this.contentEl.appendChild(wrap);
      return;
    }

    // Overview Tab
    const metaSection = document.createElement("div");
    metaSection.className = "drawer-section";
    const h3Meta = document.createElement("h3");
    h3Meta.textContent = "Session Properties";
    metaSection.appendChild(h3Meta);

    const grid = document.createElement("div");
    grid.className = "drawer-grid";

    const stats = [
      { label: "Database", val: row.database },
      { label: "User", val: row.username },
      { label: "Application", val: row.application_name || "—" },
      { label: "Client Address", val: row.client || "—" },
      { label: "Duration", val: humanDuration(row.duration_secs) },
      { label: "Transaction Age", val: row.xact_age_secs !== null ? humanDuration(row.xact_age_secs) : "none" },
      { label: "Wait Event", val: row.wait_event ?? "None" },
      { label: "SSL Connection", val: row.ssl ? `${row.ssl_version ?? "TLS"} (${row.ssl_cipher ?? "encrypted"})` : "Unencrypted" },
    ];

    for (const s of stats) {
      const statEl = document.createElement("div");
      statEl.className = "drawer-stat";
      const lbl = document.createElement("span");
      lbl.className = "drawer-stat-label";
      lbl.textContent = s.label;
      const val = document.createElement("span");
      val.className = "drawer-stat-val";
      val.textContent = s.val;
      statEl.appendChild(lbl);
      statEl.appendChild(val);
      grid.appendChild(statEl);
    }
    metaSection.appendChild(grid);
    this.contentEl.appendChild(metaSection);

    // SQL Query Section
    const sqlSection = document.createElement("div");
    sqlSection.className = "drawer-section";

    const sqlToolbar = document.createElement("div");
    sqlToolbar.className = "drawer-sql-toolbar";
    const sqlTitle = document.createElement("h3");
    sqlTitle.textContent = "Current Query";
    sqlToolbar.appendChild(sqlTitle);

    const copyBtn = renderCopyButton(() => row.query, (ok, chars) => {
      this.callbacks.onCopy?.(ok, chars);
    });
    sqlToolbar.appendChild(copyBtn);
    sqlSection.appendChild(sqlToolbar);

    const sqlCard = document.createElement("div");
    sqlCard.className = "drawer-sql-card";
    renderSqlInto(sqlCard, row.query);
    sqlSection.appendChild(sqlCard);

    this.contentEl.appendChild(sqlSection);
  }

  private renderSessionActions(row: ActivityRow): void {
    this.actionsEl.replaceChildren();
    if (!this.adminEnabled()) return;

    const cancelBtn = document.createElement("button");
    cancelBtn.className = "btn-warning";
    cancelBtn.textContent = `Cancel Query (${row.pid})`;
    cancelBtn.addEventListener("click", () => {
      this.callbacks.onAdmin?.("cancel", row);
    });

    const killBtn = document.createElement("button");
    killBtn.className = "btn-danger";
    killBtn.textContent = `Terminate Backend (${row.pid})`;
    killBtn.addEventListener("click", () => {
      this.callbacks.onAdmin?.("terminate", row);
    });

    this.actionsEl.appendChild(cancelBtn);
    this.actionsEl.appendChild(killBtn);
  }

  // ── Table (Schema) rendering ───────────────────────────────────────
  private renderTableTabs(): void {
    this.tabsEl.replaceChildren();
    const tabs = [
      { id: "columns", label: "Columns & Constraints" },
      { id: "indexes", label: "Indexes & Bloat" },
      { id: "stats", label: "Table Statistics" },
    ];

    for (const t of tabs) {
      const btn = document.createElement("button");
      btn.className = `drawer-tab ${this.currentTab === t.id ? "active" : ""}`;
      btn.textContent = t.label;
      btn.addEventListener("click", () => {
        this.currentTab = t.id;
        for (const b of this.tabsEl.querySelectorAll(".drawer-tab")) {
          b.classList.remove("active");
        }
        btn.classList.add("active");
        this.renderTableContent();
      });
      this.tabsEl.appendChild(btn);
    }
  }

  private renderTableContent(): void {
    this.contentEl.replaceChildren();
    if (!this.currentTable) return;
    const { stat, detail, schema } = this.currentTable;

    if (this.currentTab === "columns") {
      const colSection = document.createElement("div");
      colSection.className = "drawer-section";
      const h3 = document.createElement("h3");
      h3.textContent = "Columns";
      colSection.appendChild(h3);

      if (detail) {
        const colLines = columnDetailLines(detail);
        const pre = document.createElement("pre");
        pre.className = "drawer-sql-card";
        pre.textContent = colLines.length ? colLines.join("\n") : "(no columns found)";
        colSection.appendChild(pre);

        const constrSection = document.createElement("div");
        constrSection.className = "drawer-section";
        const h3c = document.createElement("h3");
        h3c.textContent = "Constraints & Foreign Keys";
        constrSection.appendChild(h3c);
        const constrLines = ownConstraintLines(detail);
        const refLines = referencingConstraintLines(detail);
        const allConstr = [...constrLines, ...refLines.map((r) => `referenced by: ${r}`)];
        const preC = document.createElement("pre");
        preC.className = "drawer-sql-card";
        preC.textContent = allConstr.length ? allConstr.join("\n") : "(no constraints found)";
        constrSection.appendChild(preC);
        this.contentEl.appendChild(colSection);
        this.contentEl.appendChild(constrSection);
      } else {
        const p = document.createElement("p");
        p.className = "placeholder";
        p.textContent = "Loading detailed structure from PostgreSQL...";
        colSection.appendChild(p);
        this.contentEl.appendChild(colSection);
      }
      return;
    }

    if (this.currentTab === "indexes") {
      const idxSection = document.createElement("div");
      idxSection.className = "drawer-section";
      const h3 = document.createElement("h3");
      h3.textContent = "Indexes & Index Bloat";
      idxSection.appendChild(h3);

      const bloatRows: BloatRow[] = (schema.index_bloat ?? []).filter(
        (b) => b.schema === stat.schema && b.table === stat.name,
      );

      if (bloatRows.length === 0) {
        const p = document.createElement("p");
        p.className = "placeholder";
        p.textContent = "No index bloat estimates collected for this table.";
        idxSection.appendChild(p);
      } else {
        const grid = document.createElement("div");
        grid.className = "drawer-grid";
        for (const b of bloatRows) {
          const statEl = document.createElement("div");
          statEl.className = "drawer-stat";
          const lbl = document.createElement("span");
          lbl.className = "drawer-stat-label";
          lbl.textContent = b.name;
          const val = document.createElement("span");
          val.className = "drawer-stat-val";
          const pct = b.bloat_pct !== null ? `${b.bloat_pct.toFixed(1)}%` : "—";
          const bytes = b.bloat_bytes !== null ? humanBytes(b.bloat_bytes) : "—";
          val.textContent = `${pct} (${bytes})`;
          statEl.appendChild(lbl);
          statEl.appendChild(val);
          grid.appendChild(statEl);
        }
        idxSection.appendChild(grid);
      }
      this.contentEl.appendChild(idxSection);
      return;
    }

    // Stats tab
    const statsSection = document.createElement("div");
    statsSection.className = "drawer-section";
    const h3 = document.createElement("h3");
    h3.textContent = "Metrics & Health";
    statsSection.appendChild(h3);

    const grid = document.createElement("div");
    grid.className = "drawer-grid";

    const metrics = [
      { label: "Total Size", val: humanBytes(stat.total_bytes) },
      { label: "Table Size", val: humanBytes(stat.table_bytes) },
      { label: "Indexes Size", val: humanBytes(stat.index_bytes) },
      { label: "Live Tuples", val: humanCount(stat.n_live_tup) },
      { label: "Dead Tuples", val: humanCount(stat.n_dead_tup) },
      { label: "Sequential Scans", val: humanCount(stat.seq_scan) },
      { label: "Index Scans", val: stat.idx_scan !== null ? humanCount(stat.idx_scan) : "—" },
      { label: "Locks Held", val: stat.lock_count !== null ? `${stat.lock_count} (${stat.lock_waiters ?? 0} waiting)` : "None" },
    ];

    for (const m of metrics) {
      const statEl = document.createElement("div");
      statEl.className = "drawer-stat";
      const lbl = document.createElement("span");
      lbl.className = "drawer-stat-label";
      lbl.textContent = m.label;
      const val = document.createElement("span");
      val.className = "drawer-stat-val";
      val.textContent = m.val;
      statEl.appendChild(lbl);
      statEl.appendChild(val);
      grid.appendChild(statEl);
    }
    statsSection.appendChild(grid);
    this.contentEl.appendChild(statsSection);
  }

  // ── Slot rendering ─────────────────────────────────────────────────
  private renderSlotContent(slot: ReplicationSlotRow): void {
    this.contentEl.replaceChildren();

    const sec = document.createElement("div");
    sec.className = "drawer-section";
    const h3 = document.createElement("h3");
    h3.textContent = "Replication Slot Properties";
    sec.appendChild(h3);

    const grid = document.createElement("div");
    grid.className = "drawer-grid";

    const stats = [
      { label: "Slot Name", val: slot.slot_name },
      { label: "Slot Type", val: slot.slot_type },
      { label: "Plugin", val: slot.plugin ?? "—" },
      { label: "Database", val: slot.database ?? "all / physical" },
      { label: "Active", val: slot.active ? `Yes (PID ${slot.active_pid ?? "—"})` : "No" },
      { label: "Retained WAL", val: slot.retained_wal_bytes != null ? humanBytes(slot.retained_wal_bytes) : "—" },
      { label: "Consumer Lag", val: slot.consumer_lag_bytes != null ? humanBytes(slot.consumer_lag_bytes) : "—" },
      { label: "Safe Size", val: slot.safe_wal_size != null ? humanBytes(slot.safe_wal_size) : "—" },
      { label: "Restart LSN", val: slot.restart_lsn ?? "—" },
      { label: "Confirmed Flush LSN", val: slot.confirmed_flush_lsn ?? "—" },
      { label: "xmin Age", val: slot.xmin_age != null ? humanCount(slot.xmin_age) : "—" },
      { label: "Catalog xmin Age", val: slot.catalog_xmin_age != null ? humanCount(slot.catalog_xmin_age) : "—" },
      { label: "Two-Phase Commit", val: slot.two_phase != null ? (slot.two_phase ? "Yes" : "No") : "—" },
      { label: "Conflicting", val: slot.conflicting ? "YES (Recovery Conflict)" : "No" },
      { label: "Invalidated", val: slot.invalidated ? String(slot.invalidated) : "No" },
    ];

    for (const s of stats) {
      const statEl = document.createElement("div");
      statEl.className = "drawer-stat";
      const lbl = document.createElement("span");
      lbl.className = "drawer-stat-label";
      lbl.textContent = s.label;
      const val = document.createElement("span");
      val.className = "drawer-stat-val";
      val.textContent = s.val;
      statEl.appendChild(lbl);
      statEl.appendChild(val);
      grid.appendChild(statEl);
    }
    sec.appendChild(grid);
    this.contentEl.appendChild(sec);
  }

  // ── Statement rendering ────────────────────────────────────────────
  private renderStatementContent(stmt: StatementRow): void {
    this.contentEl.replaceChildren();

    const statsSec = document.createElement("div");
    statsSec.className = "drawer-section";
    const h3Stats = document.createElement("h3");
    h3Stats.textContent = "Execution Profile";
    statsSec.appendChild(h3Stats);

    const grid = document.createElement("div");
    grid.className = "drawer-grid";

    const stats = [
      { label: "Total Time", val: humanMs(stmt.total_exec_ms) },
      { label: "Mean Time", val: humanMs(stmt.mean_exec_ms) },
      { label: "Calls", val: humanCount(stmt.calls) },
      { label: "Rows Returned", val: humanCount(stmt.rows) },
      { label: "Buffer Hit %", val: `${((stmt.shared_blks_hit / Math.max(1, stmt.shared_blks_hit + stmt.shared_blks_read)) * 100).toFixed(1)}%` },
      { label: "Shared Dirtied / Written", val: `${humanCount(stmt.shared_blks_dirtied)} / ${humanCount(stmt.shared_blks_written)}` },
      { label: "Temp Spill (written)", val: humanBytes(stmt.temp_blks_written * 8192) },
      { label: "I/O Read Time", val: stmt.blk_read_time_ms !== null ? humanMs(stmt.blk_read_time_ms) : "—" },
      { label: "I/O Write Time", val: stmt.blk_write_time_ms !== null ? humanMs(stmt.blk_write_time_ms) : "—" },
      { label: "WAL Generated", val: stmt.wal_bytes !== null ? humanBytes(stmt.wal_bytes) : "—" },
    ];

    for (const s of stats) {
      const statEl = document.createElement("div");
      statEl.className = "drawer-stat";
      const lbl = document.createElement("span");
      lbl.className = "drawer-stat-label";
      lbl.textContent = s.label;
      const val = document.createElement("span");
      val.className = "drawer-stat-val";
      val.textContent = s.val;
      statEl.appendChild(lbl);
      statEl.appendChild(val);
      grid.appendChild(statEl);
    }
    statsSec.appendChild(grid);
    this.contentEl.appendChild(statsSec);

    // SQL Section
    const sqlSec = document.createElement("div");
    sqlSec.className = "drawer-section";

    const sqlToolbar = document.createElement("div");
    sqlToolbar.className = "drawer-sql-toolbar";
    const sqlTitle = document.createElement("h3");
    sqlTitle.textContent = "Normalized Query Text";
    sqlToolbar.appendChild(sqlTitle);

    const copyBtn = renderCopyButton(() => stmt.query, (ok, chars) => {
      this.callbacks.onCopy?.(ok, chars);
    });
    sqlToolbar.appendChild(copyBtn);
    sqlSec.appendChild(sqlToolbar);

    const sqlCard = document.createElement("div");
    sqlCard.className = "drawer-sql-card";
    renderSqlInto(sqlCard, stmt.query);
    sqlSec.appendChild(sqlCard);

    this.contentEl.appendChild(sqlSec);
  }
}
