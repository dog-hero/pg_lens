// Macro Lens: vitals cards rendered from ServerVitals.

import type { CheckpointerStats, LockCapacity, ServerVitals, VacuumClusterAge } from "./types";
import { humanBytes, humanCount, humanDuration, humanPercent } from "./format";
import { ageSeverity } from "./vacuum";
import { checkpointerCard } from "./checkpointer";
import { lockCapacitySeverity } from "./lock_capacity";

interface Card {
  label: string;
  value: string;
  detail: string;
  /** 0..1 meter under the value; null hides the meter. */
  meter: number | null;
  /** Extra class when the metric deserves attention. */
  tone: "" | "warn" | "bad";
}

/**
 * F2's warning chip: only present once the cluster's XID wraparound
 * distance has crossed yellow/red — absent (no extra card) while healthy,
 * so the vitals row never grows for a non-issue.
 */
function vacuumCard(age: VacuumClusterAge | null): Card | null {
  if (age === null) return null;
  const sev = ageSeverity(age.max_age_xids);
  if (sev === "") return null;
  return {
    label: "XID wraparound",
    value: `${humanCount(age.max_age_xids)} xids`,
    detail: `worst db: ${age.worst_database} — VACUUM attention needed`,
    meter: null,
    tone: sev,
  };
}

/**
 * F4's checkpointer/bgwriter card. `null` (before the first poll of a
 * session) renders a calm collecting-state card instead of being omitted —
 * the card slot is always present so the layout doesn't jump.
 */
function checkpointCard(cp: CheckpointerStats | null): Card {
  if (cp === null) {
    return {
      label: "Checkpoints",
      value: "…",
      detail: "collecting checkpointer stats…",
      meter: null,
      tone: "",
    };
  }
  const card = checkpointerCard(cp);
  return {
    label: "Checkpoints",
    value: card.perMin,
    detail: `${card.pressure} · ${card.buffersPerSec} · avg ${card.avgWriteSync}`,
    meter: null,
    tone: card.severity,
  };
}

/**
 * v0.11's lock-table pressure card. `null` (collection failed this tick, or
 * no poll yet) renders a calm collecting-state card instead of being
 * omitted — same "always present" rule as `checkpointCard`.
 */
function lockCapacityCard(lc: LockCapacity | null): Card {
  if (lc === null) {
    return {
      label: "Lock table",
      value: "…",
      detail: "collecting lock-table stats…",
      meter: null,
      tone: "",
    };
  }
  const sev = lockCapacitySeverity(lc.used_fraction);
  return {
    label: "Lock table",
    value: `${lc.locks_held} / ${lc.capacity_slots} (${humanPercent(lc.used_fraction)})`,
    detail: `max_locks_per_transaction=${lc.max_locks_per_xact} · max_connections=${lc.max_connections} · max_prepared_transactions=${lc.max_prepared_xacts}`,
    meter: lc.used_fraction,
    tone: sev,
  };
}

function cards(
  v: ServerVitals,
  vacuumAge: VacuumClusterAge | null,
  checkpointer: CheckpointerStats | null,
  lockCapacity: LockCapacity | null,
): Card[] {
  const saturation =
    v.max_connections > 0 ? v.connections_total / v.max_connections : 0;
  const warning = vacuumCard(vacuumAge);
  return [
    ...(warning ? [warning] : []),
    {
      label: "Connections",
      value: `${v.connections_total} / ${v.max_connections}`,
      detail: `${v.active} active · ${v.idle} idle · ${v.idle_in_transaction} idle-in-tx · ${v.waiting} waiting`,
      meter: saturation,
      tone: saturation >= 0.9 ? "bad" : saturation >= 0.7 ? "warn" : "",
    },
    lockCapacityCard(lockCapacity),
    {
      label: "TPS",
      value: humanCount(v.tps),
      detail: "commits + rollbacks / s",
      meter: null,
      tone: "",
    },
    {
      label: "Cache hit",
      value: humanPercent(v.cache_hit_ratio),
      detail: "blks_hit / (hit + read)",
      meter: v.cache_hit_ratio,
      tone: v.cache_hit_ratio < 0.9 ? "warn" : "",
    },
    {
      label: "Deadlocks / temp",
      value: humanCount(v.deadlocks),
      detail: `${humanCount(v.temp_files)} temp files · ${humanBytes(v.temp_bytes)}`,
      meter: null,
      tone: v.deadlocks > 0 ? "bad" : "",
    },
    {
      label: "Server",
      value: `PG ${v.server_version}`,
      detail: `up ${humanDuration(v.uptime_secs)}`,
      meter: null,
      tone: "",
    },
    checkpointCard(checkpointer),
  ];
}

export function renderVitals(
  container: HTMLElement,
  v: ServerVitals,
  vacuumAge: VacuumClusterAge | null = null,
  checkpointer: CheckpointerStats | null = null,
  lockCapacity: LockCapacity | null = null,
): void {
  container.replaceChildren(
    ...cards(v, vacuumAge, checkpointer, lockCapacity).map((card) => {
      const el = document.createElement("div");
      el.className = card.tone === "" ? "card" : `card ${card.tone}`;
      const meter =
        card.meter === null
          ? ""
          : `<div class="meter"><div class="meter-fill" style="width:${(
              Math.min(1, Math.max(0, card.meter)) * 100
            ).toFixed(1)}%"></div></div>`;
      el.innerHTML = `
        <div class="card-label">${card.label}</div>
        <div class="card-value">${escapeHtml(card.value)}</div>
        ${meter}
        <div class="card-detail">${escapeHtml(card.detail)}</div>`;
      return el;
    }),
  );
}

function escapeHtml(text: string): string {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}
