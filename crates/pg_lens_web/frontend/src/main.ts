// pg_lens web frontend entrypoint: wires the SSE stream to the vitals
// cards, the uPlot history chart, and the sortable activity table.

import "./style.css";
import type { AdminActionResult, ActivityRow, DbSnapshot, PollerStatus } from "./types";
import {
  fetchConfig,
  requestAdmin,
  requestDbSwitch,
  requestSchemaRefresh,
  requestTableDetail,
  type AdminKind,
} from "./actions";
import { populateDbSwitcher } from "./db_switcher";
import { renderVitals } from "./vitals";
import { HistoryChart } from "./chart";
import {
  cacheHitReadoutSeverity,
  formatPinAge,
  formatReadoutTime,
  lockPressureReadoutSeverity,
  readoutAtIndex,
  resolvePinnedIndex,
  type ReadoutPoint,
} from "./scrubber";
import type { SnapshotHistory } from "./types";
import { ActivityTable } from "./table";
import { SchemaLens } from "./schema";
import { IndexAdvisor } from "./index-advisor";
import { SequencesPanel } from "./sequences";
import { VacuumPanel } from "./vacuum-panel";
import { StatementsLens } from "./statements";
import { initBlocksLens } from "./blocks-lens";
import { initProgressLens } from "./progress-lens";
import { initRecordsLens } from "./records-lens";
import { renderReplication } from "./replication";
import { renderWaits, renderWaitsList } from "./waits";
import { renderOldestXact } from "./xact_age";
import { renderIdleSessions } from "./idle_sessions";
import {
  clearToken,
  openStream,
  probeAuth,
  storeToken,
  storedToken,
  type StreamHandle,
} from "./stream";
import { loadStoredTheme, nextTheme, resolveInitialTheme, saveTheme, type Theme } from "./theme";
import {
  filterInputIdForPanel,
  isEditableTag,
  isExportKey,
  isRecordKey,
  isSchemaRefreshKey,
  tabIdForKey,
} from "./keyboard";
import { humanCount } from "./format";

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

// The token in use for the live connection (null = open server). Admin
// controls only appear when it is set.
let activeToken: string | null = null;
// Read-only mode (`GET /api/config`, refreshed per connection): defense in
// depth ONLY — hides/disables the buttons, but the server's `/api/admin/*`
// handler is the real, unconditional gate (see pg_lens_web::admin).
let readOnly = false;

const scrubReadout = el<HTMLDivElement>("scrub-readout");
const scrubReadoutTime = el<HTMLSpanElement>("scrub-readout-time");
const scrubReadoutPinnedHint = el<HTMLSpanElement>("scrub-readout-pinned-hint");
const scrubReadoutTps = el<HTMLElement>("scrub-readout-tps");
const scrubReadoutSessions = el<HTMLElement>("scrub-readout-sessions");
const scrubReadoutConns = el<HTMLElement>("scrub-readout-conns");
const scrubReadoutCache = el<HTMLElement>("scrub-readout-cache");
const scrubReadoutLock = el<HTMLElement>("scrub-readout-lock");
const scrubReadoutXidWrap = el<HTMLSpanElement>("scrub-readout-xid-wrap");
const scrubReadoutXid = el<HTMLElement>("scrub-readout-xid");
const scrubUnpinBtn = el<HTMLButtonElement>("scrub-unpin");

const chart = new HistoryChart(el<HTMLDivElement>("chart"), {
  onHover: (idx) => {
    if (pinnedEpochMs !== null) return; // pinned readout holds until unpinned
    const r = readoutAtIndex(currentHistory, idx);
    if (r === null) hideReadout();
    else renderReadout(r, false);
  },
  onClick: (idx) => {
    if (pinnedEpochMs !== null) {
      unpinScrub();
      return;
    }
    const r = readoutAtIndex(currentHistory, idx);
    if (r !== null) pinScrub(r);
  },
});
const table = new ActivityTable(
  el<HTMLTableElement>("activity"),
  document.getElementById("activity-filter") as HTMLInputElement | null,
  document.getElementById("activity-count"),
  {
    adminEnabled: () => activeToken !== null && !readOnly,
    onAdmin: (kind, row) => void onAdmin(kind, row),
    onCopy: (ok, chars) => onCopyResult(ok, chars),
  },
);
const vitalsContainer = el<HTMLElement>("vitals");

// ── history time-scrubber (v0.14) ─────────────────────────────────────────
// `currentHistory` is whatever the chart is currently plotting (kept in
// sync with every snapshot so the chart's hover/click callbacks — fired
// from uPlot's own event loop, not ours — can resolve a data index into a
// moment). `pinnedEpochMs` identifies the pinned moment by TIMESTAMP, not
// index: the ring buffer shifts under incoming SSE snapshots, so only the
// timestamp survives across updates (see scrubber.ts's resolvePinnedIndex).
let currentHistory: SnapshotHistory = { cap: 0, points: [] };
let pinnedEpochMs: number | null = null;

function pctText(pct: number | null): string {
  return pct === null ? "—" : `${pct.toFixed(1)}%`;
}

function renderReadout(r: ReadoutPoint, pinned: boolean): void {
  scrubReadoutTime.textContent = formatReadoutTime(r.epochMs);
  scrubReadoutTps.textContent = humanCount(r.tps);
  scrubReadoutSessions.textContent = humanCount(r.activeSessions);
  scrubReadoutConns.textContent = humanCount(r.connectionsTotal);
  scrubReadoutCache.textContent = pctText(r.cacheHitPct);
  scrubReadoutCache.className = cacheHitReadoutSeverity(r.cacheHitPct);
  scrubReadoutLock.textContent = pctText(r.lockPressurePct);
  scrubReadoutLock.className = lockPressureReadoutSeverity(r.lockPressurePct);
  if (r.oldestXidAge === null) {
    scrubReadoutXidWrap.hidden = true;
  } else {
    scrubReadoutXidWrap.hidden = false;
    scrubReadoutXid.textContent = humanCount(r.oldestXidAge);
  }
  scrubReadoutPinnedHint.hidden = !pinned;
  if (pinned) {
    scrubReadoutPinnedHint.textContent = `(pinned ${formatPinAge(r.epochMs, Date.now())})`;
  }
  scrubUnpinBtn.hidden = !pinned;
  scrubReadout.hidden = false;
  scrubReadout.classList.toggle("pinned", pinned);
}

function hideReadout(): void {
  scrubReadout.hidden = true;
}

function pinScrub(r: ReadoutPoint): void {
  pinnedEpochMs = r.epochMs;
  chart.setPinMarker(r.epochMs / 1000);
  vitalsContainer.classList.add("scrub-pinned");
  renderReadout(r, true);
}

function unpinScrub(): void {
  pinnedEpochMs = null;
  chart.setPinMarker(null);
  vitalsContainer.classList.remove("scrub-pinned");
  hideReadout();
}

scrubUnpinBtn.addEventListener("click", unpinScrub);

/** Called once per rendered snapshot: while pinned, re-resolves the pinned
 * timestamp against the freshly-streamed history — gracefully unpinning
 * ("moment aged out") once it scrolls out of the 1h ring, and otherwise just
 * refreshing the "(pinned Xm ago)" hint (the readout's own values are frozen
 * — a pinned `HistoryPoint` never changes once pushed). */
function refreshPinnedScrub(): void {
  if (pinnedEpochMs === null) return;
  const idx = resolvePinnedIndex(currentHistory, pinnedEpochMs);
  if (idx === null) {
    showToast("Pinned moment aged out of the history window", true);
    unpinScrub();
    return;
  }
  const r = readoutAtIndex(currentHistory, idx);
  if (r !== null) renderReadout(r, true);
}
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
const schemaLens = new SchemaLens(
  el<HTMLTableElement>("schema"),
  el<HTMLParagraphElement>("schema-staleness"),
  el<HTMLParagraphElement>("schema-warning"),
  el<HTMLParagraphElement>("schema-placeholder"),
  document.getElementById("schema-filter") as HTMLInputElement | null,
  (oid, schema, name) => {
    // v0.15: fire-and-forget — the response rides the normal snapshot/SSE
    // stream as `DbSnapshot.table_detail`, picked up by the next
    // `schemaLens.update()` call (see below).
    void requestTableDetail(activeToken, oid, schema, name);
  },
  document.getElementById("schema-partitions-toggle") as HTMLInputElement | null,
  (tableName) => {
    selectTab("tab-indexes");
    indexAdvisor.setFilter(tableName);
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
);

const blocksLens = initBlocksLens(el<HTMLElement>("blocks-panel"));
const progressLens = initProgressLens(el<HTMLElement>("progress-panel"));
const recordsLens = initRecordsLens(
  el<HTMLElement>("records-panel"),
  () => activeToken,
  () => readOnly,
);

// Tab switcher (U1: top-level tabs, mirroring the TUI's lenses —
// Macro/Micro stay merged into "Activity" here, vitals cards + chart stay
// visible on all of them; only the bottom panel swaps).
const tabs: Array<[HTMLButtonElement, HTMLElement]> = [
  [el<HTMLButtonElement>("tab-activity"), el<HTMLElement>("activity-panel")],
  [el<HTMLButtonElement>("tab-blocks"), el<HTMLElement>("blocks-panel")],
  [el<HTMLButtonElement>("tab-replication"), el<HTMLElement>("replication-panel")],
  [el<HTMLButtonElement>("tab-schema"), el<HTMLElement>("schema-panel")],
  [el<HTMLButtonElement>("tab-indexes"), el<HTMLElement>("indexes-panel")],
  [el<HTMLButtonElement>("tab-queries"), el<HTMLElement>("queries-panel")],
  [el<HTMLButtonElement>("tab-progress"), el<HTMLElement>("progress-panel")],
  [el<HTMLButtonElement>("tab-records"), el<HTMLElement>("records-panel")],
];
/** Switches to the tab whose button has `id === tabId` (no-op if unknown —
 * used by both the click handlers below and the `1`-`5` keyboard shortcuts). */
function selectTab(tabId: string): void {
  for (const [button, panel] of tabs) {
    const selected = button.id === tabId;
    button.setAttribute("aria-selected", String(selected));
    panel.hidden = !selected;
  }
  if (tabId === "tab-records") {
    void recordsLens.load();
  }
}

/** The panel element of whichever tab is currently selected (drives the
 * `/` filter-focus shortcut — each panel has at most one filter input). */
function activePanelId(): string | null {
  return tabs.find(([button]) => button.getAttribute("aria-selected") === "true")?.[1].id ?? null;
}

for (const [button] of tabs) {
  button.addEventListener("click", () => selectTab(button.id));
}

// ── keyboard navigation (v0.13 ROADMAP "Web keyboard navigation") ────────
// `1`-`5` jump tabs, `/` focuses the active panel's filter input, `Esc`
// blurs whatever's focused. Suppressed while a text-consuming element
// already has focus (Esc is the one exception — it must still blur).
window.addEventListener("keydown", (event) => {
  const active = document.activeElement;
  const editing = active instanceof HTMLElement && isEditableTag(active.tagName);
  if (event.key === "Escape") {
    // v0.14: Esc also unpins a scrubbed moment — checked before the blur so
    // both happen on one keypress regardless of what else has focus.
    if (pinnedEpochMs !== null) unpinScrub();
    if (active instanceof HTMLElement) active.blur();
    return;
  }
  if (editing) return;

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

  if (event.metaKey || event.ctrlKey || event.altKey) return;
  // v0.14: while a moment is pinned, Left/Right steps it one history sample
  // at a time — a lightweight way to walk through an incident tick by tick.
  if (pinnedEpochMs !== null && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
    const idx = resolvePinnedIndex(currentHistory, pinnedEpochMs);
    if (idx !== null) {
      const nextIdx =
        event.key === "ArrowLeft" ? Math.max(0, idx - 1) : Math.min(currentHistory.points.length - 1, idx + 1);
      const r = readoutAtIndex(currentHistory, nextIdx);
      if (r !== null) {
        event.preventDefault();
        pinScrub(r);
      }
    }
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

// UI-side freeze (the web twin of the TUI's Space): while paused, incoming
// snapshots are parked (last-wins) and applied on resume — the poller keeps
// running, this is purely a display freeze.
let paused = false;
let pending: DbSnapshot | null = null;
// Dedupe key for admin-action feedback (the poller re-stamps its latest
// result on every snapshot).
let lastAdminSeen = 0;

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

/** v0.16 (Part C): shared copy-button result handler for every lens's
 * expanded detail (Micro/Query/Schema) — honest wording either way (never
 * an unconditional "copied!"; see `clipboard.ts`'s doc comment). */
function onCopyResult(ok: boolean, chars: number): void {
  if (ok) {
    showToast(`Copied ${chars} chars to clipboard`);
  } else {
    showToast("Copy failed — select the text manually", true);
  }
}

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

function renderSnapshot(snapshot: DbSnapshot): void {
  renderStatus(snapshot.status);
  renderVitals(
    vitalsContainer,
    snapshot.vitals,
    snapshot.schema?.vacuum_cluster_age ?? null,
    snapshot.checkpointer,
    snapshot.lock_capacity,
    snapshot.history,
    snapshot.io_stats,
    snapshot.wal,
    snapshot.slru ?? null,
  );
  renderReplication(
    replicationBody,
    replicationPlaceholder,
    snapshot.replication,
    snapshot.replication_slots,
    snapshot.wal,
    snapshot.conflicts ?? null,
    snapshot.publications ?? null,
    snapshot.subscriptions ?? null,
  );
  currentHistory = snapshot.history;
  chart.update(snapshot.history);
  refreshPinnedScrub();
  // Top waits: aggregated over the FULL activity set (never the filtered
  // subset — it answers "what is the server stuck on"), mirroring the
  // TUI's strip above the activity table.
  renderWaits(waitsStrip, snapshot.activity);
  // U3: the complete ranked list, collapsed under the activity table (the
  // strip above only ever shows the top few).
  renderWaitsList(waitsDetail, waitsDetailSummary, waitsList, snapshot.activity);
  // v0.11: idle connection / connection-age census, collapsed under the
  // activity table like the waits list — the pool-exhaustion suspects.
  renderIdleSessions(idleDetail, idleDetailSummary, idleList, snapshot.idle_sessions);
  // v0.9: oldest open transaction, hidden on calm snapshots — the same
  // "quiet unless something's wrong" contract as the waits strip.
  renderOldestXact(xactHeadline, xactHeadlineAge, xactHeadlineMeta, xactHeadlineState, snapshot.activity);
  table.update(snapshot.activity, snapshot.locks, snapshot.ddl_progress);
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
  // v0.13: current database, prominent regardless of whether the switcher
  // itself has anything to offer (fixes the documented drift — it used to
  // be buried in `serverInfo`'s trailing text).
  currentDb.textContent = v.database;
  if (!switching) {
    populateDbSwitcher(dbSwitcher, snapshot.databases, v.database);
  }
}

// v0.13: true while a switch request is in flight — the dropdown is left
// alone until the next snapshot confirms the new database (no optimistic
// update; see `onDbSwitch`).
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

/** Surface an admin action's outcome once (deduped by at_epoch_ms). */
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

// ── light/dark theme toggle (v0.13 redesign) ──────────────────────────────
// Default dark (matches every prior screenshot/demo and the TUI's own
// always-dark terminal); the explicit choice persists in localStorage and
// wins on every later visit. See theme.ts for the pure decision logic.
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
  // Defense in depth only: the table already hides the Actions column while
  // `readOnly` is true (see `adminEnabled` above), so this only fires if
  // that check was somehow bypassed — the server's own refusal below is
  // what actually matters either way.
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
  // Best-effort: a failed fetch defaults to `readOnly = false` (fail open on
  // the UI side only — `/api/admin/*` still refuses server-side whenever the
  // server was actually started read-only, regardless of what this reports).
  void fetchConfig(token).then((cfg) => {
    readOnly = cfg.readOnly;
    readOnlyBadge.hidden = !readOnly;
    // The Actions column depends on `adminEnabled()`, which now reads
    // `readOnly` too — re-render just the head in case this resolved after
    // the first snapshot already drew it (the common, fast case never
    // notices: `fetchConfig` and the stream's first frame race, but nothing
    // depends on which wins).
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

// Boot: probe first so a token-protected server shows the prompt right away
// instead of an opaque failing EventSource.
void probeAuth(storedToken()).then((verdict) => {
  if (verdict === "unauthorized") {
    clearToken();
    showTokenPrompt(false);
  } else {
    connect(storedToken());
  }
});
