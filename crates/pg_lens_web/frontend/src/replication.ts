// Macro dashboard: replication panel (primary senders / standby receiver).
// Mirrors the TUI's Macro Lens panel — same lag severity thresholds and the
// same "hide when a primary has no replicas" rule.

import type {
  DatabaseConflicts,
  PublicationRow,
  ReplicationInfo,
  ReplicationSlotRow,
  SubscriptionRow,
  WalReceiverRow,
  WalSenderRow,
  WalStats,
} from "./types.ts";
import { humanBytes, humanCount, humanDuration } from "./format.ts";
import { walBuffersFullSeverity, walGenerationText } from "./wal.ts";

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
 * `slot_severity` exactly.
 */
export function slotSeverity(slot: ReplicationSlotRow): Severity {
  if (slot.invalidated) return "bad";
  if (slot.wal_status === "unreserved" || slot.wal_status === "lost") return "bad";
  const max_xmin_age = Math.max(slot.xmin_age ?? 0, slot.catalog_xmin_age ?? 0);
  if (max_xmin_age > 50_000_000) return "bad";
  if (!slot.active) {
    const retained = slot.retained_wal_bytes ?? 0;
    if (retained > 10 * 1024 * 1024 * 1024) return "bad";
    if (retained > 0) return "warn";
  }
  if (max_xmin_age > 10_000_000) return "warn";
  return "";
}

export function slotRow(slot: ReplicationSlotRow): HTMLDivElement {
  const sev = slotSeverity(slot);
  const retained =
    slot.retained_wal_bytes !== null ? humanBytes(slot.retained_wal_bytes) : "—";
  const status = slot.wal_status ?? "—";
  const activeText = slot.active
    ? slot.active_pid !== null && slot.active_pid !== undefined
      ? `active (pid ${slot.active_pid})`
      : "active"
    : "inactive";
  const cells: { text: string; cls?: string }[] = [
    { text: `slot ${slot.slot_name}/${slot.slot_type}`, cls: "repl-name" },
    { text: `${activeText} (${status})`, cls: "repl-state" },
    { text: `retained: ${retained}`, cls: `repl-lag ${sev}`.trim() },
  ];
  if (slot.consumer_lag_bytes !== null && slot.consumer_lag_bytes !== undefined) {
    cells.push({ text: `lag: ${humanBytes(slot.consumer_lag_bytes)}`, cls: "repl-state" });
  }
  if (slot.safe_wal_size !== null && slot.safe_wal_size !== undefined) {
    cells.push({ text: `safe: ${humanBytes(slot.safe_wal_size)}`, cls: "repl-state" });
  }
  if (slot.xmin_age !== null && slot.xmin_age !== undefined && slot.xmin_age > 0) {
    cells.push({
      text: `xmin age: ${humanCount(slot.xmin_age)}`,
      cls: slot.xmin_age > 10_000_000 ? `repl-lag ${sev}`.trim() : "repl-state",
    });
  }
  if (slot.invalidated) {
    cells.push({ text: `[invalidated: ${slot.invalidated}]`, cls: "repl-lag bad" });
  }

  const details: string[] = [];
  if (slot.plugin) details.push(`plugin: ${slot.plugin}`);
  if (slot.database) details.push(`db: ${slot.database}`);
  if (slot.temporary) details.push("temporary");
  if (slot.application_name || slot.client_addr) {
    const client = slot.application_name ?? "client";
    const addr = slot.client_addr ? ` (${slot.client_addr})` : "";
    details.push(`${client}${addr}`);
  }
  if (slot.restart_lsn) details.push(`restart: ${slot.restart_lsn}`);
  if (slot.confirmed_flush_lsn) details.push(`flush: ${slot.confirmed_flush_lsn}`);
  if (slot.catalog_xmin_age !== null && slot.catalog_xmin_age !== undefined && slot.catalog_xmin_age > 0) {
    details.push(`catalog xmin: ${humanCount(slot.catalog_xmin_age)}`);
  }
  if (slot.two_phase !== null && slot.two_phase !== undefined) {
    details.push(`2PC: ${slot.two_phase ? "yes" : "no"}`);
  }
  if (slot.conflicting) {
    details.push("! CONFLICTS WITH RECOVERY");
  }

  if (details.length > 0) {
    cells.push({ text: details.join(" · "), cls: "repl-detail" });
  }

  const r = row(cells);
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
  const sev = lagSeverity(s.total_lag_bytes ?? s.replay_lag_bytes, s.replay_lag_secs);
  const syncDesc = s.sync_priority > 0
    ? `${s.state} / ${s.sync_state} (prio ${s.sync_priority})`
    : `${s.state} / ${s.sync_state}`;
  const cells = [
    { text: `${s.application_name}/${s.client}`, cls: "repl-name" },
    { text: syncDesc, cls: "repl-state" },
    {
      text: `lag: ${lagText(s.replay_lag_bytes, s.replay_lag_secs)}`,
      cls: `repl-lag ${sev}`.trim(),
    },
  ];

  if (s.sent_lag_bytes !== null || s.write_lag_bytes !== null || s.flush_lag_bytes !== null) {
    const sent = s.sent_lag_bytes !== null ? humanBytes(s.sent_lag_bytes) : "0 B";
    const write = s.write_lag_bytes !== null ? humanBytes(s.write_lag_bytes) : "0 B";
    const flush = s.flush_lag_bytes !== null ? humanBytes(s.flush_lag_bytes) : "0 B";
    cells.push({ text: `sent: ${sent} · write: ${write} · flush: ${flush}`, cls: "repl-state" });
  }
  if (s.total_lag_bytes !== null) {
    cells.push({ text: `total: ${humanBytes(s.total_lag_bytes)}`, cls: "repl-name" });
  }

  const r = row(cells);
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
  const cells = [
    { text: "standby", cls: "repl-name" },
    { text: `${rc.status} · from ${upstream}`, cls: "repl-state" },
    {
      text: `replay lag: ${lagText(rc.replay_lag_bytes, rc.replay_lag_secs)}`,
      cls: `repl-lag ${sev}`.trim(),
    },
  ];
  if (rc.is_paused) {
    const state = rc.pause_state ?? "paused";
    cells.push({ text: `[${state}]`, cls: "repl-lag bad" });
  }
  const r = row(cells);
  if (sev) r.classList.add(`lag-${sev}`);
  return r;
}

function calmRow(text: string): HTMLDivElement {
  const div = document.createElement("div");
  div.className = "repl-row repl-state";
  div.textContent = text;
  return div;
}

/** v0.16's WAL Generation row. */
function walRow(wal: WalStats): HTMLDivElement {
  const sev = walBuffersFullSeverity(wal);
  const div = document.createElement("div");
  div.className = `repl-row repl-wal ${sev}`.trim();
  div.textContent = walGenerationText(wal);
  return div;
}

/** v0.19: One-line summary of standby recovery conflicts (pg_stat_database_conflicts). */
export function conflictsText(c: DatabaseConflicts): string {
  const rateText =
    c.conflicts_per_sec !== null && c.conflicts_per_sec > 0
      ? ` (${c.conflicts_per_sec.toFixed(1)}/s)`
      : "";
  return `conflicts: ${humanCount(c.confl_total)}${rateText} · lock ${humanCount(c.confl_lock)} · snapshot ${humanCount(c.confl_snapshot)} · deadlock ${humanCount(c.confl_deadlock)} · pin ${humanCount(c.confl_bufferpin)} · tblspc ${humanCount(c.confl_tablespace)}`;
}

export function conflictsSeverity(c: DatabaseConflicts): Severity {
  if (c.conflicts_per_sec !== null && c.conflicts_per_sec > 0) return "bad";
  if (c.confl_total > 0) return "warn";
  return "";
}

function conflictsRow(c: DatabaseConflicts): HTMLDivElement {
  const sev = conflictsSeverity(c);
  const div = document.createElement("div");
  div.className = `repl-row repl-conflicts ${sev}`.trim();
  div.textContent = `standby recovery ${conflictsText(c)}`;
  return div;
}

function section(title: string, elements: HTMLElement[]): HTMLDivElement {
  const div = document.createElement("div");
  div.className = "repl-section";
  const h3 = document.createElement("h3");
  h3.textContent = title;
  div.appendChild(h3);
  for (const el of elements) {
    div.appendChild(el);
  }
  return div;
}

export function publicationRow(p: PublicationRow): HTMLDivElement {
  const ops: string[] = [];
  if (p.pubinsert) ops.push("ins");
  if (p.pubupdate) ops.push("upd");
  if (p.pubdelete) ops.push("del");
  if (p.pubtruncate) ops.push("trunc");
  const opsStr = ops.length ? ops.join(",") : "none";
  const tablesStr = p.all_tables
    ? "all tables"
    : `${p.table_count} table${p.table_count === 1 ? "" : "s"}`;
  const cells: { text: string; cls?: string }[] = [
    { text: `pub ${p.pubname}`, cls: "repl-name" },
    { text: `(${p.owner})`, cls: "repl-state" },
    { text: `tables: ${tablesStr}`, cls: "repl-state" },
    { text: `ops: [${opsStr}]`, cls: "repl-state" },
  ];
  if (p.pubviaroot) {
    cells.push({ text: "(via_root)", cls: "repl-state" });
  }
  if (!p.all_tables && p.published_tables && p.published_tables.length > 0) {
    cells.push({
      text: `tables: [${p.published_tables.join(", ")}]`,
      cls: "repl-detail",
    });
  }
  return row(cells);
}

export function subscriptionRow(s: SubscriptionRow): HTMLDivElement {
  const hasErrors = (s.apply_error_count ?? 0) > 0 || (s.sync_error_count ?? 0) > 0;
  const sev: Severity = hasErrors ? "bad" : !s.enabled ? "warn" : "";
  const statusStr = s.enabled ? "enabled" : "disabled";
  const workerStr = s.worker_pid !== null ? `worker: pid ${s.worker_pid}` : "worker: idle";
  const lsnStr = s.received_lsn ?? "—";
  const pubsStr = s.publications.join(",");

  const cells: { text: string; cls?: string }[] = [
    { text: `sub ${s.subname}`, cls: "repl-name" },
    { text: `(${s.owner})`, cls: "repl-state" },
    { text: statusStr, cls: `repl-state ${sev}`.trim() },
    { text: workerStr, cls: "repl-state" },
    { text: `pubs: [${pubsStr}]`, cls: "repl-state" },
    { text: `recv: ${lsnStr}`, cls: "repl-state" },
    { text: `tables: ${s.ready_tables}/${s.total_tables} ready`, cls: "repl-state" },
  ];
  if (s.sync_tables > 0) {
    cells.push({ text: `(${s.sync_tables} syncing)`, cls: "repl-lag warn" });
  }
  if (s.apply_error_count && s.apply_error_count > 0) {
    cells.push({ text: `apply errors: ${s.apply_error_count}`, cls: "repl-lag bad" });
  }
  if (s.sync_error_count && s.sync_error_count > 0) {
    cells.push({ text: `sync errors: ${s.sync_error_count}`, cls: "repl-lag bad" });
  }

  const details: string[] = [];
  if (s.publisher_host || s.publisher_dbname) {
    const host = s.publisher_host ?? "—";
    const port = s.publisher_port ? `:${s.publisher_port}` : "";
    const db = s.publisher_dbname ? `/${s.publisher_dbname}` : "";
    details.push(`conn: ${host}${port}${db}`);
  }
  if (s.slot_name) {
    details.push(`slot: ${s.slot_name}`);
  }
  if (s.sync_commit) {
    details.push(`sync_commit: ${s.sync_commit}`);
  }
  if (s.streaming_mode) {
    details.push(`streaming: ${s.streaming_mode}`);
  }
  if (s.binary_mode !== undefined && s.binary_mode !== null) {
    details.push(`binary: ${s.binary_mode ? "on" : "off"}`);
  }
  if (s.two_phase !== undefined && s.two_phase !== null) {
    details.push(`2PC: ${s.two_phase ? "on" : "off"}`);
  }
  if (s.last_msg_receipt_secs !== null && s.last_msg_receipt_secs !== undefined) {
    details.push(`last msg: ${humanDuration(s.last_msg_receipt_secs)} ago`);
  }
  if (s.syncing_table_names && s.syncing_table_names.length > 0) {
    details.push(`syncing tables: [${s.syncing_table_names.join(", ")}]`);
  }

  if (details.length > 0) {
    cells.push({ text: details.join(" · "), cls: "repl-detail" });
  }

  const r = row(cells);
  if (sev) r.classList.add(`lag-${sev}`);
  return r;
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
 * Renders the Replication Lens into `body`.
 */
export function renderReplication(
  body: HTMLElement,
  placeholder: HTMLElement,
  repl: ReplicationInfo | null,
  slots: ReplicationSlotRow[] | null,
  wal: WalStats | null = null,
  conflicts: DatabaseConflicts | null = null,
  publications: PublicationRow[] | null = null,
  subscriptions: SubscriptionRow[] | null = null,
): void {
  if (
    repl === null &&
    slots === null &&
    conflicts === null &&
    publications === null &&
    subscriptions === null
  ) {
    placeholder.hidden = false;
    body.replaceChildren();
    return;
  }
  placeholder.hidden = true;

  const sections: HTMLElement[] = [];

  // 1. Physical Replication (Role) & WAL
  const roleRows: HTMLElement[] = [];
  if (wal !== null) {
    roleRows.push(walRow(wal));
  }
  if (conflicts !== null) {
    roleRows.push(conflictsRow(conflicts));
  }
  if (repl && "Primary" in repl) {
    if (repl.Primary.senders.length === 0) {
      roleRows.push(calmRow("primary · no replicas connected"));
    } else {
      for (const s of repl.Primary.senders) roleRows.push(senderRow(s));
    }
  } else if (repl && "Standby" in repl) {
    if (repl.Standby.receiver) {
      roleRows.push(receiverRow(repl.Standby.receiver));
    } else {
      roleRows.push(calmRow("standby · waiting for a WAL sender…"));
    }
  } else {
    roleRows.push(calmRow("role: collecting…"));
  }
  sections.push(section("Physical Replication (Role)", roleRows));

  // 2. Publications (pg_publication) - dedicated section
  if (publications && publications.length > 0) {
    const pubRows = publications.map((p) => publicationRow(p));
    sections.push(section("Publications (pg_publication)", pubRows));
  }

  // 3. Subscriptions (pg_subscription) - dedicated section
  if (subscriptions && subscriptions.length > 0) {
    const subRows = subscriptions.map((s) => subscriptionRow(s));
    sections.push(section("Subscriptions (pg_subscription)", subRows));
  }

  // 4. Replication Slots
  const slotRows: HTMLElement[] = [];
  if (slots && slots.length > 0) {
    for (const s of sortedSlots(slots)) slotRows.push(slotRow(s));
  } else {
    slotRows.push(calmRow("no replication slots"));
  }
  sections.push(section("Replication Slots", slotRows));

  body.replaceChildren(...sections);
}
