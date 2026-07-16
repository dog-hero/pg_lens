// pg_lens web frontend entrypoint: wires the SSE stream to the vitals
// cards, the uPlot history chart, and the sortable activity table.

import "./style.css";
import type { AdminActionResult, ActivityRow, DbSnapshot, PollerStatus } from "./types";
import { requestAdmin, requestSchemaRefresh, type AdminKind } from "./actions";
import { renderVitals } from "./vitals";
import { HistoryChart } from "./chart";
import { ActivityTable } from "./table";
import { SchemaLens } from "./schema";
import { IndexAdvisor } from "./index-advisor";
import { VacuumPanel } from "./vacuum-panel";
import { StatementsLens } from "./statements";
import { renderReplication } from "./replication";
import { renderWaits } from "./waits";
import {
  clearToken,
  openStream,
  probeAuth,
  storeToken,
  storedToken,
  type StreamHandle,
} from "./stream";

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (node === null) throw new Error(`missing #${id}`);
  return node as T;
}

const serverInfo = el<HTMLSpanElement>("server-info");
const connState = el<HTMLSpanElement>("conn-state");
const statusBanner = el<HTMLDivElement>("status-banner");
const tokenOverlay = el<HTMLDivElement>("token-overlay");
const tokenForm = el<HTMLFormElement>("token-form");
const tokenInput = el<HTMLInputElement>("token-input");
const tokenError = el<HTMLParagraphElement>("token-error");

const toast = el<HTMLSpanElement>("toast");
const pauseBtn = el<HTMLButtonElement>("pause-btn");
const schemaRefreshBtn = el<HTMLButtonElement>("schema-refresh-btn");

// The token in use for the live connection (null = open server). Admin
// controls only appear when it is set.
let activeToken: string | null = null;

const chart = new HistoryChart(el<HTMLDivElement>("chart"));
const table = new ActivityTable(
  el<HTMLTableElement>("activity"),
  document.getElementById("activity-filter") as HTMLInputElement | null,
  document.getElementById("activity-count"),
  {
    adminEnabled: () => activeToken !== null,
    onAdmin: (kind, row) => void onAdmin(kind, row),
  },
);
const vitalsContainer = el<HTMLElement>("vitals");
const waitsStrip = el<HTMLDivElement>("waits-strip");
const replicationPanel = el<HTMLElement>("replication-panel");
const replicationBody = el<HTMLElement>("replication");
const schemaLens = new SchemaLens(
  el<HTMLTableElement>("schema"),
  el<HTMLParagraphElement>("schema-staleness"),
  el<HTMLParagraphElement>("schema-warning"),
  el<HTMLParagraphElement>("schema-placeholder"),
);
const indexAdvisor = new IndexAdvisor(
  el<HTMLTableElement>("indexes"),
  el<HTMLParagraphElement>("indexes-staleness"),
  el<HTMLParagraphElement>("indexes-warning"),
  el<HTMLParagraphElement>("indexes-placeholder"),
);
// Schema sub-tabs (F3): Tables (the panel's original content) vs Indexes —
// nested inside the Schema tab, mirroring the TUI's `i` toggle. Both keep
// polling from the same snapshot; only the visible view differs.
const schemaSubtabs: Array<[HTMLButtonElement, HTMLElement]> = [
  [el<HTMLButtonElement>("schema-view-tables"), el<HTMLElement>("schema-tables-view")],
  [el<HTMLButtonElement>("schema-view-indexes"), el<HTMLElement>("schema-indexes-view")],
];
for (const [button] of schemaSubtabs) {
  button.addEventListener("click", () => {
    for (const [other, view] of schemaSubtabs) {
      const selected = other === button;
      other.classList.toggle("active", selected);
      other.setAttribute("aria-pressed", String(selected));
      view.hidden = !selected;
    }
  });
}
const vacuumPanel = new VacuumPanel(
  el<HTMLParagraphElement>("vacuum-cluster"),
  el<HTMLUListElement>("vacuum-tables"),
  el<HTMLParagraphElement>("vacuum-progress"),
);
const statementsLens = new StatementsLens(
  el<HTMLTableElement>("statements"),
  el<HTMLParagraphElement>("statements-staleness"),
  el<HTMLParagraphElement>("statements-warning"),
  el<HTMLParagraphElement>("statements-placeholder"),
  el<HTMLDivElement>("statements-unavailable"),
);

// Tab switcher: vitals cards + chart stay visible on both tabs; only the
// bottom panel (Activity table vs Schema table) swaps.
const tabs: Array<[HTMLButtonElement, HTMLElement]> = [
  [el<HTMLButtonElement>("tab-activity"), el<HTMLElement>("activity-panel")],
  [el<HTMLButtonElement>("tab-schema"), el<HTMLElement>("schema-panel")],
  [el<HTMLButtonElement>("tab-queries"), el<HTMLElement>("queries-panel")],
];
for (const [button] of tabs) {
  button.addEventListener("click", () => {
    for (const [other, panel] of tabs) {
      const selected = other === button;
      other.setAttribute("aria-selected", String(selected));
      panel.hidden = !selected;
    }
  });
}

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

function onSnapshot(snapshot: DbSnapshot): void {
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
  );
  renderReplication(
    replicationPanel,
    replicationBody,
    snapshot.replication,
    snapshot.replication_slots,
  );
  chart.update(snapshot.history);
  // Top waits: aggregated over the FULL activity set (never the filtered
  // subset — it answers "what is the server stuck on"), mirroring the
  // TUI's strip above the activity table.
  renderWaits(waitsStrip, snapshot.activity);
  table.update(snapshot.activity, snapshot.locks);
  schemaLens.update(snapshot.schema, snapshot.vitals.database);
  indexAdvisor.update(snapshot.schema, snapshot.vitals.database);
  vacuumPanel.update(snapshot.schema, snapshot.vacuum_progress);
  statementsLens.update(snapshot.statements, snapshot.vitals.database);
  announceAdmin(snapshot.last_admin_action);
  const v = snapshot.vitals;
  serverInfo.textContent = `PG ${v.server_version} · ${v.connections_total}/${v.max_connections} conns`;
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
  pauseBtn.textContent = paused ? "Resume" : "Pause";
  pauseBtn.classList.toggle("active", paused);
  connState.dataset["paused"] = String(paused);
  if (!paused && pending !== null) {
    renderSnapshot(pending);
    pending = null;
  }
});

schemaRefreshBtn.addEventListener("click", () => {
  void requestSchemaRefresh(activeToken).then((ok) => {
    showToast(
      ok ? "Schema refresh requested" : "Schema refresh failed",
      !ok,
    );
  });
});

async function onAdmin(kind: AdminKind, row: ActivityRow): Promise<void> {
  const verb = kind === "cancel" ? "Cancel query on" : "Terminate backend";
  if (!window.confirm(`${verb} PID ${row.pid} (${row.username}@${row.database})?`)) {
    return;
  }
  const result = await requestAdmin(activeToken, kind, row.pid);
  if (result.status === 403) {
    showToast("Admin actions require the server to have a token set", true);
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
