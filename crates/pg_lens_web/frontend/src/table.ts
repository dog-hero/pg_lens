// Micro Lens: activity table with client-side sort and B/W status markers.
//
// Mirrors the TUI's micro_lens.rs conventions: status column `S` shows `B`
// when the pid appears in DbSnapshot::locks (blocked — red tint, wins) and
// `W` when wait_event is non-null (waiting — yellow tint).
//
// v0.16: the WHOLE row is colored, pg_activity-style (Part A) — see
// `activityRowClass` for the exact precedence (blocked wins, then a
// running-query duration override, then plain waiting, then the session
// state's own base color), mirroring `ui/micro_lens.rs::row_severity_style`
// exactly. The query cell itself is now plain text in the table row — SQL
// keyword highlighting moved into the expanded detail row only (Part B),
// which also carries a copy-to-clipboard button (Part C).

import type { ActivityRow, DdlProgressRow, LockRow } from "./types";
import { humanDuration } from "./format.ts";
import { renderSqlInto } from "./sql.ts";
import { xactAgeSeverity } from "./xact_age.ts";
import { blockingChain, renderBlockingChain } from "./blocking.ts";
import { renderCopyButton } from "./clipboard.ts";
import type { AdminKind } from "./actions";

/** Above this running-query duration the Duration column's color indicates
 * severity — bad (red) past this, warn (yellow) past `ROW_DURATION_WARN_SECS`
 * — mirroring the TUI's `ROW_DURATION_BAD_SECS`/`ROW_DURATION_WARN_SECS`
 * exactly. Only ever applies to `state === "active"` sessions: an idle (or
 * idle-in-transaction) session stays dim/idle color. */
export const ROW_DURATION_BAD_SECS = 30;
export const ROW_DURATION_WARN_SECS = 10;

/** pg_activity-style per-column state color class —
 * mirrors the TUI's `state_color` mapping exactly. Unknown/rare states
 * (`fastpath function call`, `disabled`, or any future addition)
 * map to `""` (neutral default). */
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

/** Severity class applied strictly to the Duration column:
 * active sessions turn bad (>30s), warn (>10s), or ok (<=10s).
 * Idle sessions remain calm/dim. */
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

/** Color class for wait events: Lock events are highlighted as red/bold,
 * other wait events are yellow, and empty is dim. */
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
  private ddlProgress: DdlProgressRow[] = [];
  /** pid of the row whose expanded detail (full highlighted query + copy
   * button, plus the wait-for chain when blocked) is open, if any — v0.9's
   * blocked-only chain toggle generalized (v0.16) to every row, mirroring
   * the TUI's `Enter`-to-open detail panel. */
  private expandedPid: number | null = null;
  private filter = "";
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly count: HTMLElement | null;
  /** True when admin actions are available (a token is active). */
  private readonly adminEnabled: () => boolean;
  /** Invoked when a row's Cancel/Kill button is pressed. */
  private readonly onAdmin: ((kind: AdminKind, row: ActivityRow) => void) | null;
  /** Invoked after a copy-button click resolves — lets the caller show a
   * toast (see `main.ts`'s `showToast`). */
  private readonly onCopy: ((ok: boolean, chars: number) => void) | null;

  constructor(
    table: HTMLTableElement,
    filterInput?: HTMLInputElement | null,
    count?: HTMLElement | null,
    opts?: {
      adminEnabled?: () => boolean;
      onAdmin?: (kind: AdminKind, row: ActivityRow) => void;
      onCopy?: (ok: boolean, chars: number) => void;
    },
  ) {
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.count = count ?? null;
    this.adminEnabled = opts?.adminEnabled ?? (() => false);
    this.onAdmin = opts?.onAdmin ?? null;
    this.onCopy = opts?.onCopy ?? null;
    if (filterInput) {
      filterInput.addEventListener("input", () => {
        this.filter = filterInput.value.trim().toLowerCase();
        this.renderBody();
      });
    }
    this.renderHead();
  }

  update(activity: ActivityRow[], locks: LockRow[], ddlProgress?: DdlProgressRow[] | null): void {
    this.rows = activity;
    this.locks = locks;
    this.blocked = new Set(locks.map((lock) => lock.pid));
    this.ddlProgress = ddlProgress ?? [];
    // A pid can stop being on screen between polls (query finished, session
    // gone) — drop a stale expansion rather than pointing at nothing.
    if (this.expandedPid !== null && !this.rows.some((r) => r.pid === this.expandedPid)) {
      this.expandedPid = null;
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

      // Duration column: time-based coloring applied strictly to this column
      const durationTd = document.createElement("td");
      durationTd.classList.add("col-duration", "num");
      durationTd.classList.add(durationSeverityClass(row.state, row.duration_secs));
      durationTd.textContent = humanDuration(row.duration_secs);
      tr.append(durationTd);

      // Xact column: age of the open transaction ("—" when none), tinted
      // by the same severity the oldest-xact headline uses —
      // idle-in-transaction reads worse than an equally-old active query.
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

      // Query cell: neutral plain text — SQL keyword highlighting
      // is kept in the expanded detail row below.
      const query = document.createElement("td");
      query.classList.add("col-query", "query");
      query.title = row.query;
      query.textContent = row.query;
      tr.append(query);
      if (this.showActions()) {
        tr.append(this.actionsCell(row));
      }
      // v0.16: every row is clickable — toggles an expanded detail row
      // (full highlighted query + copy button, plus the wait-for chain when
      // blocked) right below, mirroring the TUI's `Enter` detail panel.
      // Generalized (v0.9 used to gate this on `isBlocked` only).
      tr.classList.add("row-toggle");
      tr.addEventListener("click", (e) => {
        // Don't hijack clicks on the Cancel/Kill/Copy buttons.
        if (e.target instanceof HTMLButtonElement) return;
        this.expandedPid = this.expandedPid === row.pid ? null : row.pid;
        this.renderBody();
      });
      trs.push(tr);
      if (this.expandedPid === row.pid) {
        trs.push(this.detailRow(row, isBlocked, colCount));
      }
    }
    this.tbody.replaceChildren(...trs);
  }

  /** Expanded detail row (v0.16): the full, SQL-highlighted query (Part B —
   * this is the ONLY place in the Micro Lens that still highlights), a copy
   * button (Part C), and — when the row is blocked — the wait-for chain
   * that used to be the whole of this sub-row pre-v0.16. */
  private detailRow(row: ActivityRow, isBlocked: boolean, colCount: number): HTMLTableRowElement {
    const tr = document.createElement("tr");
    tr.classList.add("activity-detail");
    const td = document.createElement("td");
    td.colSpan = colCount;

    const metaWrap = document.createElement("div");
    metaWrap.classList.add("activity-detail-meta");

    const secSpan = document.createElement("span");
    secSpan.classList.add("meta-item", "meta-security");
    if (row.ssl) {
      const ver = row.ssl_version ?? "TLS";
      const cipher = row.ssl_cipher ? ` (${row.ssl_cipher})` : "";
      secSpan.textContent = `Security: 🔒 ${ver}${cipher} (encrypted)`;
      secSpan.classList.add("sec-ssl");
    } else if (row.client === "local") {
      secSpan.textContent = "Security: unix-socket (local)";
      secSpan.classList.add("sec-local");
    } else {
      secSpan.textContent = "Security: ⚠️ plain (unencrypted)";
      secSpan.classList.add("sec-plain");
    }
    metaWrap.append(secSpan);

    const ddl = this.ddlProgress.find((d) => d.pid === row.pid);
    if (ddl) {
      const ddlSpan = document.createElement("span");
      ddlSpan.classList.add("meta-item", "meta-ddl");
      const pct = ddl.progress_pct !== null ? `${ddl.progress_pct.toFixed(1)}%` : "in progress";
      const detailSuffix = ddl.detail ? ` [${ddl.detail}]` : "";
      ddlSpan.textContent = `⚙️ DDL: ${ddl.command} on ${ddl.relation} — ${ddl.phase} (${ddl.current_step}/${ddl.total_step}) | ${pct}${detailSuffix}`;
      metaWrap.append(ddlSpan);
    }
    td.append(metaWrap);

    const pre = document.createElement("pre");
    renderSqlInto(pre, row.query);
    td.append(pre);
    if (this.onCopy !== null) {
      td.append(
        renderCopyButton(
          () => row.query,
          (ok, chars) => this.onCopy?.(ok, chars),
        ),
      );
    }
    if (isBlocked) {
      const chain = blockingChain(row.pid, this.locks);
      if (chain !== null) {
        const chainWrap = document.createElement("div");
        chainWrap.classList.add("blocking-chain-row");
        chainWrap.append(renderBlockingChain(chain));
        td.append(chainWrap);
      }
    }
    tr.append(td);
    return tr;
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
