// pg_lens web frontend entrypoint: wires the SSE stream to the vitals
// cards, the uPlot history chart, and the sortable activity table.

import "./style.css";
import type { DbSnapshot, PollerStatus } from "./types";
import { renderVitals } from "./vitals";
import { HistoryChart } from "./chart";
import { ActivityTable } from "./table";
import { SchemaLens } from "./schema";
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

const chart = new HistoryChart(el<HTMLDivElement>("chart"));
const table = new ActivityTable(el<HTMLTableElement>("activity"));
const vitalsContainer = el<HTMLElement>("vitals");
const schemaLens = new SchemaLens(
  el<HTMLTableElement>("schema"),
  el<HTMLParagraphElement>("schema-staleness"),
  el<HTMLParagraphElement>("schema-warning"),
  el<HTMLParagraphElement>("schema-placeholder"),
);

// Tab switcher: vitals cards + chart stay visible on both tabs; only the
// bottom panel (Activity table vs Schema table) swaps.
const tabs: Array<[HTMLButtonElement, HTMLElement]> = [
  [el<HTMLButtonElement>("tab-activity"), el<HTMLElement>("activity-panel")],
  [el<HTMLButtonElement>("tab-schema"), el<HTMLElement>("schema-panel")],
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

function renderSnapshot(snapshot: DbSnapshot): void {
  renderStatus(snapshot.status);
  renderVitals(vitalsContainer, snapshot.vitals);
  chart.update(snapshot.history);
  table.update(snapshot.activity, snapshot.locks);
  schemaLens.update(snapshot.schema, snapshot.vitals.database);
  const v = snapshot.vitals;
  serverInfo.textContent = `PG ${v.server_version} · ${v.connections_total}/${v.max_connections} conns`;
}

function connect(token: string | null): void {
  stream?.close();
  setConnState("connecting");
  stream = openStream(token, {
    onSnapshot: renderSnapshot,
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
