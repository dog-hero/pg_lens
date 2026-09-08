// pg_lens web frontend entrypoint: wires the SSE stream to the 9 lenses,
// compact health ribbon, inspector drawer, and command palette.

import "./style.css";
import type { AdminActionResult, ActivityRow, DatabaseRow, DbSnapshot, PollerStatus } from "./types.ts";
import {
  fetchConfig,
  requestAdmin,
  requestDbSwitch,
  requestSchemaRefresh,
  requestTableDetail,
  type AdminKind,
} from "./actions.ts";
import { populateDbSwitcher } from "./db_switcher.ts";
import { MacroLens } from "./macro.ts";
import { ActivityTable, type StateFilter } from "./table.ts";
import { SchemaLens } from "./schema.ts";
import { IndexAdvisor } from "./index-advisor.ts";
import { SequencesPanel } from "./sequences.ts";
import { VacuumPanel } from "./vacuum-panel.ts";
import { StatementsLens } from "./statements.ts";
import { initBlocksLens } from "./blocks-lens.ts";
import { initProgressLens } from "./progress-lens.ts";
import { initRecordsLens } from "./records-lens.ts";
import { renderReplication } from "./replication.ts";
import { renderWaits, renderWaitsList } from "./waits.ts";
import { renderOldestXact } from "./xact_age.ts";
import { renderIdleSessions } from "./idle_sessions.ts";
import {
  clearToken,
  openStream,
  probeAuth,
  storeToken,
  storedToken,
  type StreamHandle,
} from "./stream.ts";
import { loadStoredTheme, nextTheme, resolveInitialTheme, saveTheme, type Theme } from "./theme.ts";
import {
  filterInputIdForPanel,
  isEditableTag,
  isExportKey,
  isHelpKey,
  isPaletteKey,
  isRecordKey,
  isSchemaRefreshKey,
  tabIdForKey,
} from "./keyboard.ts";
import { InspectorDrawer } from "./drawer.ts";
import { CommandPalette, type PaletteAction } from "./command_palette.ts";
import { humanBytes, humanDuration } from "./format.ts";

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (node === null) throw new Error(`missing #${id}`);
  return node as T;
}

const serverInfo = el<HTMLSpanElement>("server-info");
const currentDb = el<HTMLSpanElement>("current-db");
const dbSwitcher = el<HTMLSelectElement>("db-switcher");
const dbSwitchStatus = el<HTMLSpanElement>("db-switch-status");
const readOnlyBadge = el<HTMLSpanElement>("read-only-badge");
const connState = el<HTMLSpanElement>("conn-state");
const statusBanner = el<HTMLDivElement>("status-banner");
const tokenOverlay = el<HTMLDivElement>("token-overlay");
const tokenForm = el<HTMLFormElement>("token-form");
const tokenInput = el<HTMLInputElement>("token-input");
const tokenError = el<HTMLParagraphElement>("token-error");

const toast = el<HTMLSpanElement>("toast");
const pauseBtn = el<HTMLButtonElement>("pause-btn");
const pauseBtnIcon = pauseBtn.querySelector("use");
const pauseBtnLabel = pauseBtn.querySelector("span");
const recordBtn = el<HTMLButtonElement>("record-btn");
const recordBtnLabel = el<HTMLSpanElement>("record-btn-label");
const exportBtn = el<HTMLButtonElement>("export-btn");
const schemaRefreshBtn = el<HTMLButtonElement>("schema-refresh-btn");
const themeToggleBtn = el<HTMLButtonElement>("theme-toggle");
const themeToggleIcon = themeToggleBtn.querySelector("use");
const paletteToggleBtn = el<HTMLButtonElement>("palette-toggle");
const brandLink = el<HTMLDivElement>("brand-link");

// Health ribbon (visible on Lenses 2 to 9)
const healthRibbon = el<HTMLDivElement>("health-ribbon");
const ribbonDb = el<HTMLElement>("ribbon-db");
const ribbonPg = el<HTMLElement>("ribbon-pg");
const ribbonConns = el<HTMLElement>("ribbon-conns");
const ribbonActive = el<HTMLElement>("ribbon-active");
const ribbonWaiting = el<HTMLElement>("ribbon-waiting");
const ribbonTps = el<HTMLElement>("ribbon-tps");
const ribbonCache = el<HTMLElement>("ribbon-cache");
const ribbonXactWrap = el<HTMLElement>("ribbon-xact-wrap");
const ribbonXact = el<HTMLElement>("ribbon-xact");

let activeToken: string | null = null;
let readOnly = false;
let toastTimer: number | undefined;

function showToast(message: string, isError = false): void {
  toast.textContent = message;
  toast.dataset["kind"] = isError ? "error" : "ok";
  toast.hidden = false;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
  }, 5000);
}

function onCopyResult(ok: boolean, chars: number): void {
  if (ok) {
    showToast(`Copied ${chars} chars to clipboard`);
  } else {
    showToast("Copy failed — select text manually", true);
  }
}

// ── Components initialization ─────────────────────────────────────────────
const drawer = new InspectorDrawer(
  "inspector-drawer",
  "drawer-backdrop",
  () => activeToken !== null && !readOnly,
  {
    onAdmin: (kind, row) => void onAdmin(kind, row),
    onCopy: (ok, chars) => onCopyResult(ok, chars),
  },
);

const palette = new CommandPalette("palette-backdrop", "palette-input", "palette-list");
const macroLens = new MacroLens(el<HTMLElement>("macro-panel"));

const table = new ActivityTable(
  el<HTMLTableElement>("activity"),
  document.getElementById("activity-filter") as HTMLInputElement | null,
  document.getElementById("activity-count"),
  {
    adminEnabled: () => activeToken !== null && !readOnly,
    onAdmin: (kind, row) => void onAdmin(kind, row),
    onInspect: (row, locks) => drawer.openSession(row, locks),
  },
);

const waitsStrip = el<HTMLDivElement>("waits-strip");
const waitsDetail = el<HTMLDetailsElement>("waits-detail");
const waitsDetailSummary = el<HTMLElement>("waits-detail-summary");
const waitsList = el<HTMLUListElement>("waits-list");
const idleDetail = el<HTMLDetailsElement>("idle-detail");
const idleDetailSummary = el<HTMLElement>("idle-detail-summary");
const idleList = el<HTMLUListElement>("idle-list");
const xactHeadline = el<HTMLDivElement>("xact-headline");
const xactHeadlineAge = el<HTMLSpanElement>("xact-headline-age");
const xactHeadlineMeta = el<HTMLSpanElement>("xact-headline-meta");
const xactHeadlineState = el<HTMLSpanElement>("xact-headline-state");
const replicationBody = el<HTMLElement>("replication");
const replicationPlaceholder = el<HTMLParagraphElement>("replication-placeholder");
const replicationFilter = document.getElementById("replication-filter") as HTMLInputElement | null;

const schemaLens = new SchemaLens(
  el<HTMLTableElement>("schema"),
  el<HTMLParagraphElement>("schema-staleness"),
  el<HTMLParagraphElement>("schema-warning"),
  el<HTMLParagraphElement>("schema-placeholder"),
  document.getElementById("schema-filter") as HTMLInputElement | null,
  (oid, schema, name) => {
    void requestTableDetail(activeToken, oid, schema, name);
  },
  document.getElementById("schema-partitions-toggle") as HTMLInputElement | null,
  (tableName) => {
    selectTab("tab-indexes");
    indexAdvisor.setFilter(tableName);
  },
  (tableStat, schemaSnapshot, detail) => {
    drawer.openTable(tableStat, schemaSnapshot, detail);
  },
);

const indexAdvisor = new IndexAdvisor(
  el<HTMLTableElement>("indexes"),
  el<HTMLParagraphElement>("indexes-staleness"),
  el<HTMLParagraphElement>("indexes-warning"),
  el<HTMLParagraphElement>("indexes-placeholder"),
  document.getElementById("indexes-filter") as HTMLInputElement | null,
);

const sequencesPanel = new SequencesPanel(
  el<HTMLTableElement>("sequences"),
  el<HTMLParagraphElement>("sequences-placeholder"),
  document.getElementById("sequences-staleness"),
);

const vacuumPanel = new VacuumPanel(
  el<HTMLParagraphElement>("vacuum-cluster"),
  el<HTMLUListElement>("vacuum-tables"),
  el<HTMLParagraphElement>("vacuum-progress"),
  el<HTMLUListElement>("prepared-xacts"),
);

const statementsLens = new StatementsLens(
  el<HTMLTableElement>("statements"),
  el<HTMLParagraphElement>("statements-staleness"),
  el<HTMLParagraphElement>("statements-warning"),
  el<HTMLParagraphElement>("statements-placeholder"),
  el<HTMLDivElement>("statements-unavailable"),
  document.getElementById("statements-filter") as HTMLInputElement | null,
  (ok, chars) => onCopyResult(ok, chars),
  (stmt) => drawer.openStatement(stmt),
);

const blocksLens = initBlocksLens(el<HTMLElement>("blocks-panel"));
const progressLens = initProgressLens(el<HTMLElement>("progress-panel"));
const recordsLens = initRecordsLens(
  el<HTMLElement>("records-panel"),
  () => activeToken,
  () => readOnly,
);

// ── Tab Navigation (9 Lenses) ─────────────────────────────────────────────
const tabs: Array<[HTMLButtonElement, HTMLElement]> = [
  [el<HTMLButtonElement>("tab-macro"), el<HTMLElement>("macro-panel")],
  [el<HTMLButtonElement>("tab-activity"), el<HTMLElement>("activity-panel")],
  [el<HTMLButtonElement>("tab-blocks"), el<HTMLElement>("blocks-panel")],
  [el<HTMLButtonElement>("tab-replication"), el<HTMLElement>("replication-panel")],
  [el<HTMLButtonElement>("tab-schema"), el<HTMLElement>("schema-panel")],
  [el<HTMLButtonElement>("tab-indexes"), el<HTMLElement>("indexes-panel")],
  [el<HTMLButtonElement>("tab-queries"), el<HTMLElement>("queries-panel")],
  [el<HTMLButtonElement>("tab-progress"), el<HTMLElement>("progress-panel")],
  [el<HTMLButtonElement>("tab-records"), el<HTMLElement>("records-panel")],
];

function selectTab(tabId: string): void {
  for (const [button, panel] of tabs) {
    const selected = button.id === tabId;
    button.setAttribute("aria-selected", String(selected));
    panel.hidden = !selected;
  }

  // Health ribbon is visible on Lenses 2 to 9, hidden on Overview (Lens 1)
  healthRibbon.hidden = tabId === "tab-macro";

  if (tabId === "tab-records") {
    void recordsLens.load();
  }
}

function activePanelId(): string | null {
  return tabs.find(([button]) => button.getAttribute("aria-selected") === "true")?.[1].id ?? null;
}

for (const [button] of tabs) {
  button.addEventListener("click", () => selectTab(button.id));
}

healthRibbon.addEventListener("click", () => selectTab("tab-macro"));
brandLink.addEventListener("click", () => selectTab("tab-macro"));
paletteToggleBtn.addEventListener("click", () => palette.open());

// Activity state chips
const activityChips = el<HTMLDivElement>("activity-chips");
activityChips.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".chip");
  if (!btn) return;
  const state = btn.dataset["state"] as StateFilter;
  if (!state) return;
  for (const c of activityChips.querySelectorAll(".chip")) {
    c.classList.remove("active");
  }
  btn.classList.add("active");
  table.setStateFilter(state);
});

// Replication filter input
replicationFilter?.addEventListener("input", () => {
  if (latestSnapshot) {
    renderReplication(
      replicationBody,
      replicationPlaceholder,
      latestSnapshot.replication,
      latestSnapshot.replication_slots,
      latestSnapshot.wal,
      latestSnapshot.conflicts ?? null,
      latestSnapshot.publications ?? null,
      latestSnapshot.subscriptions ?? null,
      replicationFilter.value,
      (slot) => drawer.openSlot(slot),
    );
  }
});

// ── Command Palette Actions Setup ─────────────────────────────────────────
function updatePaletteActions(databases: DatabaseRow[] | null): void {
  const actions: PaletteAction[] = [
    // Lenses
    { id: "nav-macro", group: "Lenses", label: "Overview", shortcut: "1", iconId: "icon-macro", run: () => selectTab("tab-macro") },
    { id: "nav-activity", group: "Lenses", label: "Live Activity", shortcut: "2", iconId: "icon-activity", run: () => selectTab("tab-activity") },
    { id: "nav-blocks", group: "Lenses", label: "Blocks & Locks", shortcut: "3", iconId: "icon-blocks", run: () => selectTab("tab-blocks") },
    { id: "nav-replication", group: "Lenses", label: "Replication Lens", shortcut: "4", iconId: "icon-replication", run: () => selectTab("tab-replication") },
    { id: "nav-schema", group: "Lenses", label: "Schema & Bloat", shortcut: "5", iconId: "icon-schema", run: () => selectTab("tab-schema") },
    { id: "nav-indexes", group: "Lenses", label: "Indexes & Advisor", shortcut: "6", iconId: "icon-indexes", run: () => selectTab("tab-indexes") },
    { id: "nav-queries", group: "Lenses", label: "Queries (pg_stat_statements)", shortcut: "7", iconId: "icon-queries", run: () => selectTab("tab-queries") },
    { id: "nav-progress", group: "Lenses", label: "Progress (DDL / Maintenance)", shortcut: "8", iconId: "icon-progress", run: () => selectTab("tab-progress") },
    { id: "nav-records", group: "Lenses", label: "Records & Incident Replay", shortcut: "9", iconId: "icon-records", run: () => selectTab("tab-records") },
  ];

  // Databases
  if (databases && databases.length > 0) {
    for (const d of databases) {
      actions.push({
        id: `db-${d.name}`,
        group: "Databases",
        label: `Switch database: ${d.name}`,
        detail: d.size_bytes ? humanBytes(d.size_bytes) : undefined,
        iconId: "icon-database",
        run: () => void onDbSwitch(d.name),
      });
    }
  }

  // Quick Actions
  actions.push(
    { id: "act-pause", group: "Actions", label: paused ? "Resume live stream" : "Pause live stream", shortcut: "Space", iconId: paused ? "icon-play" : "icon-pause", run: () => pauseBtn.click() },
    { id: "act-record", group: "Actions", label: isRecording ? "Stop incident recording" : "Start incident recording", shortcut: "Shift+R", iconId: "icon-dot", run: () => recordBtn.click() },
    { id: "act-export", group: "Actions", label: "Export snapshot bookmark (JSON)", shortcut: "E", iconId: "icon-export", run: () => exportSnapshot() },
    { id: "act-refresh", group: "Actions", label: "Recollect schema & bloat", shortcut: "B", iconId: "icon-refresh", run: () => schemaRefreshBtn.click() },
    { id: "act-theme", group: "Actions", label: "Toggle Dark / Light theme", iconId: "icon-sun", run: () => themeToggleBtn.click() },
  );

  palette.setActions(actions);
}

// ── Keyboard Navigation ───────────────────────────────────────────────────
window.addEventListener("keydown", (event) => {
  const active = document.activeElement;
  const editing = active instanceof HTMLElement && isEditableTag(active.tagName);

  if (isPaletteKey(event)) {
    event.preventDefault();
    if (palette.isOpen()) palette.close();
    else palette.open();
    return;
  }

  if (event.key === "Escape") {
    if (palette.isOpen()) {
      palette.close();
      return;
    }
    if (drawer.isOpen()) {
      drawer.close();
      return;
    }
    if (macroLens.isPinned()) {
      macroLens.unpinScrub();
      return;
    }
    if (active instanceof HTMLElement) active.blur();
    return;
  }

  if (editing || palette.isOpen()) return;

  if (isRecordKey(event)) {
    event.preventDefault();
    toggleRecording();
    return;
  }
  if (isExportKey(event)) {
    event.preventDefault();
    exportSnapshot();
    return;
  }
  if (isSchemaRefreshKey(event)) {
    event.preventDefault();
    schemaRefreshBtn.click();
    return;
  }
  if (isHelpKey(event)) {
    event.preventDefault();
    palette.open();
    return;
  }

  // Row navigation (j / k or ArrowDown / ArrowUp) on Activity table
  const currentPanel = activePanelId();
  if (currentPanel === "activity-panel" && !drawer.isOpen()) {
    if (event.key === "j" || event.key === "ArrowDown") {
      event.preventDefault();
      table.selectNext();
      return;
    }
    if (event.key === "k" || event.key === "ArrowUp") {
      event.preventDefault();
      table.selectPrev();
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      table.inspectSelected();
      return;
    }
    if (event.key === "y") {
      const sel = table.getSelectedRow();
      if (sel) {
        event.preventDefault();
        void navigator.clipboard.writeText(sel.query).then(
          () => showToast(`Copied query of PID ${sel.pid}`),
          () => showToast("Failed to copy query", true),
        );
        return;
      }
    }
    if (event.key === "c" && activeToken !== null && !readOnly) {
      const sel = table.getSelectedRow();
      if (sel) {
        event.preventDefault();
        void onAdmin("cancel", sel);
        return;
      }
    }
  }

  if (event.metaKey || event.ctrlKey || event.altKey) return;

  // Pin stepping on Macro chart
  if (macroLens.isPinned() && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
    event.preventDefault();
    macroLens.stepPin(event.key === "ArrowLeft" ? -1 : 1);
    return;
  }

  const tabId = tabIdForKey(event.key);
  if (tabId !== null) {
    event.preventDefault();
    selectTab(tabId);
    return;
  }

  if (event.key === "/") {
    const panelId = activePanelId();
    const inputId = panelId === null ? null : filterInputIdForPanel(panelId);
    if (inputId !== null) {
      const input = document.getElementById(inputId);
      if (input instanceof HTMLInputElement) {
        event.preventDefault();
        input.focus();
      }
    }
  }
});

let stream: StreamHandle | null = null;

function setConnState(state: "connecting" | "live" | "reconnecting"): void {
  connState.dataset["state"] = state;
  connState.textContent = state === "live" ? "● live" : `${state}…`;
}

function renderStatus(status: PollerStatus): void {
  if (typeof status === "object" && "Error" in status) {
    statusBanner.textContent = `poller error: ${status.Error} — showing last good data`;
    statusBanner.hidden = false;
  } else if (status === "Connecting") {
    statusBanner.textContent = "connecting to PostgreSQL…";
    statusBanner.hidden = false;
  } else {
    statusBanner.hidden = true;
  }
}

let paused = false;
let pending: DbSnapshot | null = null;
let lastAdminSeen = 0;
let latestSnapshot: DbSnapshot | null = null;
let isRecording = false;
let recordedFrames: DbSnapshot[] = [];
let recordStartEpochMs = 0;
let recordTimer: number | undefined;

function formatElapsed(elapsedMs: number): string {
  const totalSec = Math.max(0, Math.floor(elapsedMs / 1000));
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${String(min).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
}

function updateRecordButton(): void {
  if (!isRecording) {
    recordBtn.classList.remove("recording");
    recordBtn.title = "Toggle incident recording mode (Shift+R / Ctrl+R)";
    recordBtnLabel.textContent = "Record";
    return;
  }
  recordBtn.classList.add("recording");
  const elapsed = formatElapsed(Date.now() - recordStartEpochMs);
  const count = recordedFrames.length;
  recordBtn.title = `Incident recording in progress (${count} frames, ${elapsed}) — click or Shift+R to stop and save`;
  recordBtnLabel.textContent = `REC ${elapsed} (${count})`;
}

function onSnapshot(snapshot: DbSnapshot): void {
  latestSnapshot = snapshot;
  if (isRecording) {
    recordedFrames.push(snapshot);
    updateRecordButton();
  }
  if (paused) {
    pending = snapshot;
    return;
  }
  renderSnapshot(snapshot);
}

function updateHealthRibbon(snapshot: DbSnapshot): void {
  const v = snapshot.vitals;
  ribbonDb.textContent = v.database;
  ribbonPg.textContent = String(v.server_version);
  ribbonConns.textContent = `${v.connections_total}/${v.max_connections}`;
  const activeCount = snapshot.activity.filter((a) => a.state === "active").length;
  const waitingCount = snapshot.activity.filter((a) => a.wait_event !== null).length;
  ribbonActive.textContent = String(activeCount);
  ribbonWaiting.textContent = String(waitingCount);
  const lastPoint = snapshot.history?.points.at(-1);
  ribbonTps.textContent = lastPoint ? lastPoint.tps.toFixed(1) : "—";
  ribbonCache.textContent = `${(v.cache_hit_ratio * 100).toFixed(1)}%`;

  const oldestXact = snapshot.activity
    .filter((a) => a.xact_age_secs !== null)
    .sort((a, b) => (b.xact_age_secs ?? 0) - (a.xact_age_secs ?? 0))[0];

  if (oldestXact && oldestXact.xact_age_secs && oldestXact.xact_age_secs > 5) {
    ribbonXactWrap.hidden = false;
    ribbonXact.textContent = humanDuration(oldestXact.xact_age_secs);
    ribbonXact.className = oldestXact.xact_age_secs > 30 ? "bad" : "warn";
  } else {
    ribbonXactWrap.hidden = true;
  }
}

function updateActivityChipCounts(snapshot: DbSnapshot): void {
  const all = snapshot.activity.length;
  const active = snapshot.activity.filter((a) => a.state === "active").length;
  const waiting = snapshot.activity.filter((a) => a.wait_event !== null).length;
  const idleTxn = snapshot.activity.filter((a) => a.state.includes("idle in transaction")).length;
  const blocked = snapshot.activity.filter((a) => snapshot.locks.some((l) => l.pid === a.pid)).length;

  const countAll = document.getElementById("chip-count-all");
  const countActive = document.getElementById("chip-count-active");
  const countWaiting = document.getElementById("chip-count-waiting");
  const countIdleTxn = document.getElementById("chip-count-idle-txn");
  const countBlocked = document.getElementById("chip-count-blocked");

  if (countAll) countAll.textContent = String(all);
  if (countActive) countActive.textContent = String(active);
  if (countWaiting) countWaiting.textContent = String(waiting);
  if (countIdleTxn) countIdleTxn.textContent = String(idleTxn);
  if (countBlocked) countBlocked.textContent = String(blocked);
}

function renderSnapshot(snapshot: DbSnapshot): void {
  renderStatus(snapshot.status);

  // 1. Macro Lens update
  macroLens.update(
    snapshot.vitals,
    snapshot.schema?.vacuum_cluster_age ?? null,
    snapshot.checkpointer,
    snapshot.lock_capacity,
    snapshot.history,
    snapshot.io_stats,
    snapshot.wal,
    snapshot.slru ?? null,
  );

  // 2. Health ribbon & topbar
  updateHealthRibbon(snapshot);
  updateActivityChipCounts(snapshot);

  // 3. Replication
  renderReplication(
    replicationBody,
    replicationPlaceholder,
    snapshot.replication,
    snapshot.replication_slots,
    snapshot.wal,
    snapshot.conflicts ?? null,
    snapshot.publications ?? null,
    snapshot.subscriptions ?? null,
    replicationFilter?.value ?? "",
    (slot) => drawer.openSlot(slot),
  );

  // 4. Waits & Oldest Xact
  renderWaits(waitsStrip, snapshot.activity);
  renderWaitsList(waitsDetail, waitsDetailSummary, waitsList, snapshot.activity);
  renderIdleSessions(idleDetail, idleDetailSummary, idleList, snapshot.idle_sessions);
  renderOldestXact(xactHeadline, xactHeadlineAge, xactHeadlineMeta, xactHeadlineState, snapshot.activity);

  // 5. Activity Table
  table.update(snapshot.activity, snapshot.locks, snapshot.ddl_progress);

  // 6. Other lenses
  schemaLens.update(snapshot.schema, snapshot.vitals.database, snapshot.table_detail);
  sequencesPanel.update(snapshot.schema?.sequences);
  indexAdvisor.update(snapshot.schema, snapshot.vitals.database);
  vacuumPanel.update(snapshot.schema, snapshot.vacuum_progress, snapshot.prepared_xacts, snapshot.ddl_progress);
  statementsLens.update(snapshot.statements, snapshot.vitals.database);
  blocksLens.update(snapshot.blocking_tree, snapshot.active_locks);
  progressLens.update(snapshot.ddl_progress, snapshot.vacuum_progress);

  announceAdmin(snapshot.last_admin_action);

  const v = snapshot.vitals;
  serverInfo.textContent = `PG ${v.server_version} · ${v.connections_total}/${v.max_connections} conns`;
  currentDb.textContent = v.database;

  if (!switching) {
    populateDbSwitcher(dbSwitcher, snapshot.databases, v.database);
  }
  updatePaletteActions(snapshot.databases);
}

let switching = false;

dbSwitcher.addEventListener("change", () => void onDbSwitch(dbSwitcher.value));

async function onDbSwitch(database: string): Promise<void> {
  switching = true;
  dbSwitcher.disabled = true;
  dbSwitchStatus.hidden = false;
  dbSwitchStatus.textContent = "switching…";
  const ok = await requestDbSwitch(activeToken, database);
  dbSwitcher.disabled = false;
  dbSwitchStatus.hidden = true;
  switching = false;
  if (!ok) {
    showToast(`Failed to switch to ${database}`, true);
  }
}

function announceAdmin(result: AdminActionResult | null): void {
  if (result === null || result.at_epoch_ms === lastAdminSeen) return;
  lastAdminSeen = result.at_epoch_ms;
  const verb = result.kind === "Cancel" ? "cancel" : "terminate";
  if ("Signalled" in result.outcome) {
    if (result.outcome.Signalled) {
      showToast(`${verb} succeeded (PID ${result.pid})`);
    } else {
      showToast(
        `PID ${result.pid} not signalled — gone or insufficient privilege`,
        true,
      );
    }
  } else {
    showToast(`${verb} PID ${result.pid} failed: ${result.outcome.Error}`, true);
  }
}

pauseBtn.addEventListener("click", () => {
  paused = !paused;
  if (pauseBtnLabel) pauseBtnLabel.textContent = paused ? "Resume" : "Pause";
  pauseBtnIcon?.setAttribute("href", paused ? "#icon-play" : "#icon-pause");
  pauseBtn.classList.toggle("active", paused);
  connState.dataset["paused"] = String(paused);
  if (!paused && pending !== null) {
    renderSnapshot(pending);
    pending = null;
  }
});

let currentTheme: Theme = resolveInitialTheme(loadStoredTheme(window.localStorage));

function applyTheme(theme: Theme): void {
  document.documentElement.dataset["theme"] = theme;
  themeToggleIcon?.setAttribute("href", theme === "dark" ? "#icon-moon" : "#icon-sun");
  themeToggleBtn.setAttribute(
    "aria-label",
    theme === "dark" ? "Switch to light theme" : "Switch to dark theme",
  );
}

applyTheme(currentTheme);
themeToggleBtn.addEventListener("click", () => {
  currentTheme = nextTheme(currentTheme);
  applyTheme(currentTheme);
  saveTheme(window.localStorage, currentTheme);
});

schemaRefreshBtn.addEventListener("click", () => {
  void requestSchemaRefresh(activeToken).then((ok) => {
    showToast(
      ok ? "Schema refresh requested" : "Schema refresh failed",
      !ok,
    );
  });
});

function downloadBlob(content: string, filename: string, mimeType: string): void {
  const blob = new Blob([content], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

function formatFilenameTimestamp(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const yr = d.getFullYear();
  const mo = pad(d.getMonth() + 1);
  const da = pad(d.getDate());
  const hr = pad(d.getHours());
  const mi = pad(d.getMinutes());
  const se = pad(d.getSeconds());
  return `${yr}${mo}${da}-${hr}${mi}${se}`;
}

function exportSnapshot(): void {
  if (latestSnapshot === null) {
    showToast("No snapshot available to export", true);
    return;
  }
  const db = latestSnapshot.vitals?.database || "pg";
  const safeDb = db.replace(/[^a-zA-Z0-9_-]/g, "_");
  const ts = formatFilenameTimestamp(new Date());
  const filename = `snapshot-${safeDb}-${ts}.json`;
  const content = JSON.stringify(latestSnapshot, null, 2);
  downloadBlob(content, filename, "application/json");
  showToast(`Exported snapshot bookmark: ${filename}`);
}

function toggleRecording(): void {
  if (isRecording) {
    isRecording = false;
    if (recordTimer !== undefined) {
      clearInterval(recordTimer);
      recordTimer = undefined;
    }
    updateRecordButton();
    if (recordedFrames.length === 0) {
      showToast("Recording stopped (0 frames captured)");
      return;
    }
    const db = latestSnapshot?.vitals?.database || "pg";
    const safeDb = db.replace(/[^a-zA-Z0-9_-]/g, "_");
    const ts = formatFilenameTimestamp(new Date());
    const filename = `rec-${safeDb}-${ts}.jsonl`;
    const jsonl = recordedFrames.map((f) => JSON.stringify(f)).join("\n") + "\n";
    downloadBlob(jsonl, filename, "application/x-ndjson");
    showToast(`Saved recording: ${filename} (${recordedFrames.length} frames)`);
    recordedFrames = [];
  } else {
    isRecording = true;
    recordedFrames = [];
    recordStartEpochMs = Date.now();
    if (latestSnapshot !== null) {
      recordedFrames.push(latestSnapshot);
    }
    updateRecordButton();
    recordTimer = window.setInterval(updateRecordButton, 1000);
    showToast("Incident recording started (Shift+R to stop)");
  }
}

recordBtn.addEventListener("click", toggleRecording);
exportBtn.addEventListener("click", exportSnapshot);

async function onAdmin(kind: AdminKind, row: ActivityRow): Promise<void> {
  if (readOnly) {
    showToast("Server is running in read-only mode: admin actions are disabled", true);
    return;
  }
  const verb = kind === "cancel" ? "Cancel query on" : "Terminate backend";
  if (!window.confirm(`${verb} PID ${row.pid} (${row.username}@${row.database})?`)) {
    return;
  }
  const result = await requestAdmin(activeToken, kind, row.pid);
  if (result.status === 403) {
    showToast(
      "Admin actions are disabled (read-only mode, or the server has no token set)",
      true,
    );
  } else if (!result.ok) {
    showToast(`Admin request failed (HTTP ${result.status || "network"})`, true);
  } else {
    showToast(`${kind} sent to PID ${row.pid}…`);
  }
}

function connect(token: string | null): void {
  stream?.close();
  activeToken = token;
  setConnState("connecting");
  void fetchConfig(token).then((cfg) => {
    readOnly = cfg.readOnly;
    readOnlyBadge.hidden = !readOnly;
    table.refreshHead();
  });
  stream = openStream(token, {
    onSnapshot,
    onStateChange: setConnState,
    onUnauthorized: () => {
      clearToken();
      showTokenPrompt(token !== null);
    },
  });
}

function showTokenPrompt(rejected: boolean): void {
  tokenError.hidden = !rejected;
  tokenOverlay.hidden = false;
  tokenInput.focus();
}

tokenForm.addEventListener("submit", (event) => {
  event.preventDefault();
  const token = tokenInput.value.trim();
  if (token === "") return;
  void probeAuth(token).then((verdict) => {
    if (verdict === "unauthorized") {
      tokenError.hidden = false;
      return;
    }
    storeToken(token);
    tokenOverlay.hidden = true;
    tokenInput.value = "";
    connect(token);
  });
});

void probeAuth(storedToken()).then((verdict) => {
  if (verdict === "unauthorized") {
    clearToken();
    showTokenPrompt(false);
  } else {
    connect(storedToken());
  }
});
