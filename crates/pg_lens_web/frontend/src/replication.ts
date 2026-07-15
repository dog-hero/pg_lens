// Macro dashboard: replication panel (primary senders / standby receiver).
// Mirrors the TUI's Macro Lens panel — same lag severity thresholds and the
// same "hide when a primary has no replicas" rule.

import type { ReplicationInfo, WalReceiverRow, WalSenderRow } from "./types";
import { humanBytes, humanDuration } from "./format";

type Severity = "" | "warn" | "bad";

/**
 * Yellow > 10 MB or > 10 s, red > 100 MB or > 60 s (either trips it). 0 bytes
 * outstanding is definitively caught up: the standby's seconds measure grows
 * on an idle primary even when in sync, so it never alarms on its own.
 */
function lagSeverity(bytes: number | null, secs: number | null): Severity {
  if (bytes === 0) return "";
  const b = bytes ?? 0;
  const s = secs ?? 0;
  if (b > 100 * 1024 * 1024 || s > 60) return "bad";
  if (b > 10 * 1024 * 1024 || s > 10) return "warn";
  return "";
}

function lagText(bytes: number | null, secs: number | null): string {
  const parts: string[] = [];
  if (bytes !== null) parts.push(humanBytes(bytes));
  if (secs !== null) parts.push(humanDuration(secs));
  return parts.length ? parts.join(" · ") : "—";
}

function row(cells: { text: string; cls?: string }[]): HTMLDivElement {
  const div = document.createElement("div");
  div.className = "repl-row";
  for (const c of cells) {
    const span = document.createElement("span");
    span.textContent = c.text;
    if (c.cls) span.className = c.cls;
    div.appendChild(span);
  }
  return div;
}

function senderRow(s: WalSenderRow): HTMLDivElement {
  const sev = lagSeverity(s.replay_lag_bytes, s.replay_lag_secs);
  const r = row([
    { text: `${s.application_name}/${s.client}`, cls: "repl-name" },
    { text: `${s.state} / ${s.sync_state}`, cls: "repl-state" },
    {
      text: `lag: ${lagText(s.replay_lag_bytes, s.replay_lag_secs)}`,
      cls: `repl-lag ${sev}`.trim(),
    },
  ]);
  if (sev) r.classList.add(`lag-${sev}`);
  return r;
}

function receiverRow(rc: WalReceiverRow): HTMLDivElement {
  const sev = lagSeverity(rc.replay_lag_bytes, rc.replay_lag_secs);
  const upstream =
    rc.sender_host !== null
      ? rc.sender_port !== null
        ? `${rc.sender_host}:${rc.sender_port}`
        : rc.sender_host
      : "upstream";
  const r = row([
    { text: "standby", cls: "repl-name" },
    { text: `${rc.status} · from ${upstream}`, cls: "repl-state" },
    {
      text: `replay lag: ${lagText(rc.replay_lag_bytes, rc.replay_lag_secs)}`,
      cls: `repl-lag ${sev}`.trim(),
    },
  ]);
  if (sev) r.classList.add(`lag-${sev}`);
  return r;
}

/**
 * Renders the replication panel into `body`, toggling `panel`'s visibility.
 * Hidden when there is nothing to show (no data yet, or a primary with no
 * replicas) — matching the TUI.
 */
export function renderReplication(
  panel: HTMLElement,
  body: HTMLElement,
  repl: ReplicationInfo | null,
): void {
  const rows: HTMLElement[] = [];
  if (repl && "Primary" in repl) {
    for (const s of repl.Primary.senders) rows.push(senderRow(s));
  } else if (repl && "Standby" in repl) {
    if (repl.Standby.receiver) {
      rows.push(receiverRow(repl.Standby.receiver));
    } else {
      const div = document.createElement("div");
      div.className = "repl-row repl-state";
      div.textContent = "standby · waiting for a WAL sender…";
      rows.push(div);
    }
  }

  if (rows.length === 0) {
    panel.hidden = true;
    body.replaceChildren();
    return;
  }
  panel.hidden = false;
  body.replaceChildren(...rows);
}
