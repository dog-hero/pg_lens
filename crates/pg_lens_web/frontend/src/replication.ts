// Macro dashboard: replication panel (primary senders / standby receiver).
// Mirrors the TUI's Macro Lens panel — same lag severity thresholds and the
// same "hide when a primary has no replicas" rule.

import type {
  ReplicationInfo,
  ReplicationSlotRow,
  WalReceiverRow,
  WalSenderRow,
} from "./types";
import { humanBytes, humanDuration } from "./format.ts";

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

/**
 * Severity of one replication slot (F2.5), mirroring the TUI's
 * `slot_severity` exactly: an INACTIVE slot that keeps retaining WAL is the
 * classic full-disk incident. Red trumps yellow — `wal_status` of
 * `unreserved`/`lost` is always red, regardless of the retained-bytes
 * reading; otherwise an inactive slot is yellow once it retains anything,
 * red past 10 GB. An active slot (even a big-retaining one — it's a live
 * replica consuming the WAL) stays calm.
 */
export function slotSeverity(slot: ReplicationSlotRow): Severity {
  if (slot.wal_status === "unreserved" || slot.wal_status === "lost") return "bad";
  if (!slot.active) {
    const retained = slot.retained_wal_bytes ?? 0;
    if (retained > 10 * 1024 * 1024 * 1024) return "bad";
    if (retained > 0) return "warn";
  }
  return "";
}

function slotRow(slot: ReplicationSlotRow): HTMLDivElement {
  const sev = slotSeverity(slot);
  const retained =
    slot.retained_wal_bytes !== null ? humanBytes(slot.retained_wal_bytes) : "—";
  const status = slot.wal_status ?? "—";
  const activeText = slot.active ? "active" : "inactive";
  const r = row([
    { text: `slot ${slot.slot_name}/${slot.slot_type}`, cls: "repl-name" },
    { text: `${activeText} (${status})`, cls: "repl-state" },
    { text: `retained: ${retained}`, cls: `repl-lag ${sev}`.trim() },
  ]);
  if (sev) r.classList.add(`lag-${sev}`);
  return r;
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

function calmRow(text: string): HTMLDivElement {
  const div = document.createElement("div");
  div.className = "repl-row repl-state";
  div.textContent = text;
  return div;
}

/** Worst-severity-first, then retained bytes descending — the web twin of
 * the TUI's `resort_replication` (see `crates/pg_lens_tui/src/app.rs`). */
function sortedSlots(slots: ReplicationSlotRow[]): ReplicationSlotRow[] {
  const rank = (s: ReplicationSlotRow): number => {
    const sev = slotSeverity(s);
    return sev === "bad" ? 0 : sev === "warn" ? 1 : 2;
  };
  return [...slots].sort((a, b) => {
    const r = rank(a) - rank(b);
    if (r !== 0) return r;
    return (b.retained_wal_bytes ?? 0) - (a.retained_wal_bytes ?? 0);
  });
}

/**
 * Renders the Replication Lens (U1: a top-level tab now, not a panel that
 * hides itself away) into `body`, toggling `placeholder` while nothing has
 * been collected yet. Unlike the Macro dashboard's compact summary, this
 * view shows EVERYTHING — every sender/receiver, every slot, no caps —
 * with a calm line standing in for each section when it has nothing to
 * report (never a bare empty table).
 */
export function renderReplication(
  body: HTMLElement,
  placeholder: HTMLElement,
  repl: ReplicationInfo | null,
  slots: ReplicationSlotRow[] | null,
): void {
  if (repl === null && slots === null) {
    placeholder.hidden = false;
    body.replaceChildren();
    return;
  }
  placeholder.hidden = true;

  const rows: HTMLElement[] = [];
  if (repl && "Primary" in repl) {
    if (repl.Primary.senders.length === 0) {
      rows.push(calmRow("primary · no replicas connected"));
    } else {
      for (const s of repl.Primary.senders) rows.push(senderRow(s));
    }
  } else if (repl && "Standby" in repl) {
    if (repl.Standby.receiver) {
      rows.push(receiverRow(repl.Standby.receiver));
    } else {
      rows.push(calmRow("standby · waiting for a WAL sender…"));
    }
  } else {
    rows.push(calmRow("role: collecting…"));
  }

  if (slots && slots.length > 0) {
    for (const s of sortedSlots(slots)) rows.push(slotRow(s));
  } else {
    rows.push(calmRow("no replication slots"));
  }

  body.replaceChildren(...rows);
}
