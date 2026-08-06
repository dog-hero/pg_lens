// Macro Lens: vitals cards rendered from ServerVitals.

import type {
  CheckpointerStats,
  IoStatRow,
  LockCapacity,
  ServerVitals,
  SnapshotHistory,
  VacuumClusterAge,
  WalStats,
} from "./types";
import { humanBytes, humanCount, humanDuration, humanPercent } from "./format";
import { ageSeverity } from "./vacuum";
import { checkpointerCard } from "./checkpointer";
import { ioStatLines } from "./io_stats";
import { lockCapacitySeverity } from "./lock_capacity";
import { walBuffersFullSeverity, walGenerationText } from "./wal";
import { TREND_LOOKBACK_TICKS, cardTrend, sampleForTrend, trendGlyph, trendTitle, trendTone } from "./trend";

interface Card {
  label: string;
  value: string;
  detail: string;
  /** 0..1 meter under the value; null hides the meter. */
  meter: number | null;
  /** Extra class when the metric deserves attention. */
  tone: "" | "warn" | "bad";
  /** v0.13: the two headline saturation gauges (Connections, Lock table)
   * render bigger and span two grid columns — real visual hierarchy instead
   * of a flat wall of same-size cards. */
  lead?: boolean;
  /** v0.14: trend arrow (now vs ~5 min ago) next to the label — `undefined`
   * on cards this doesn't apply to. */
  trend?: { glyph: string; tone: "" | "warn"; title: string };
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
 * F4's checkpointer/bgwriter card, plus v0.16's WAL generation summary
 * tacked onto the detail line (both are disk-write/buffer-pressure health,
 * thematically adjacent — mirrors the TUI's Macro Lens placement of
 * `wal_generation_line` inside the "Checkpoints / writer" panel). `cp ===
 * null` (before the first poll of a session) renders a calm collecting-state
 * card instead of being omitted — the card slot is always present so the
 * layout doesn't jump. `wal === null` (PG < 14, a restricted role, or no
 * successful collection yet) simply omits that part of the detail line.
 */
function checkpointCard(cp: CheckpointerStats | null, wal: WalStats | null): Card {
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
  // v0.13 layout fix: `card.perMin` ("0.31 timed / 0.02 req /min") is a full
  // sentence, not a headline number — cramming it into the big `.card-value`
  // slot is what clipped/overflowed the card. A short total rate is the
  // actual headline; the sentence-shaped breakdown belongs in the detail
  // line alongside pressure/buffer/write-sync, same as every other card.
  const total =
    cp.checkpoints_per_min_timed !== null && cp.checkpoints_per_min_req !== null
      ? `${(cp.checkpoints_per_min_timed + cp.checkpoints_per_min_req).toFixed(2)}/min`
      : "--/min";
  const walDetail = wal !== null ? ` · ${walGenerationText(wal)}` : "";
  const tone: Card["tone"] =
    card.severity === "warn" || (wal !== null && walBuffersFullSeverity(wal) === "warn")
      ? "warn"
      : "";
  return {
    label: "Checkpoints",
    value: total,
    detail: `${card.perMin} · ${card.pressure} · ${card.buffersPerSec} · avg ${card.avgWriteSync}${walDetail}`,
    meter: null,
    tone,
  };
}

/**
 * v0.11's lock-table pressure card. `null` (collection failed this tick, or
 * no poll yet) renders a calm collecting-state card instead of being
 * omitted — same "always present" rule as `checkpointCard`.
 */
function lockCapacityCard(
  lc: LockCapacity | null,
  trendInfo: Card["trend"],
): Card {
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
    trend: trendInfo,
  };
}

/**
 * v0.16's I/O profile card (`pg_stat_io`, PG 16+). `null` (PG < 16, or no
 * slow collection yet) means NO card at all — unlike `checkpointCard`, this
 * is genuinely absent on unsupported servers, not just "collecting", so the
 * card list must not grow for a feature the connected server can't offer.
 */
function ioStatsCard(rows: IoStatRow[] | null): Card | null {
  if (rows === null) return null;
  const lines = ioStatLines(rows);
  return {
    label: "I/O profile (pg_stat_io)",
    value: `${rows.length} source${rows.length === 1 ? "" : "s"}`,
    detail: lines.join(" · "),
    meter: null,
    tone: "",
  };
}

function cards(
  v: ServerVitals,
  vacuumAge: VacuumClusterAge | null,
  checkpointer: CheckpointerStats | null,
  lockCapacity: LockCapacity | null,
  history: SnapshotHistory,
  ioStats: IoStatRow[] | null,
  wal: WalStats | null,
): Card[] {
  const saturation =
    v.max_connections > 0 ? v.connections_total / v.max_connections : 0;
  const warning = vacuumCard(vacuumAge);

  // v0.14: trend arrows compare "now" against the sample ~5 minutes back
  // (clamped to the oldest available point on a young ring/session).
  const baseline = sampleForTrend(history, TREND_LOOKBACK_TICKS);
  const connTrend = cardTrend(v.connections_total, baseline?.connections_total ?? null);
  const cacheNow = v.cache_hit_ratio * 100;
  const cacheTrend = cardTrend(cacheNow, baseline?.cache_hit_pct ?? null);
  const lockNow = lockCapacity !== null ? lockCapacity.used_fraction * 100 : null;
  const lockTrend =
    lockCapacity !== null ? cardTrend(lockCapacity.used_fraction * 100, baseline?.lock_pressure_pct ?? null) : "flat";

  return [
    ...(warning ? [warning] : []),
    {
      label: "Connections",
      value: `${v.connections_total} / ${v.max_connections}`,
      detail: `${v.active} active · ${v.idle} idle · ${v.idle_in_transaction} idle-in-tx · ${v.waiting} waiting`,
      meter: saturation,
      tone: saturation >= 0.9 ? "bad" : saturation >= 0.7 ? "warn" : "",
      lead: true,
      trend: {
        glyph: trendGlyph(connTrend),
        tone: trendTone(connTrend, true),
        title: trendTitle(v.connections_total, baseline?.connections_total ?? null, ""),
      },
    },
    {
      ...lockCapacityCard(
        lockCapacity,
        lockNow === null
          ? undefined
          : {
              glyph: trendGlyph(lockTrend),
              tone: trendTone(lockTrend, true),
              title: trendTitle(lockNow, baseline?.lock_pressure_pct ?? null, "%"),
            },
      ),
      lead: true,
    },
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
      trend: {
        glyph: trendGlyph(cacheTrend),
        tone: trendTone(cacheTrend, false),
        title: trendTitle(cacheNow, baseline?.cache_hit_pct ?? null, "%"),
      },
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
    checkpointCard(checkpointer, wal),
    ...(ioStatsCard(ioStats) ? [ioStatsCard(ioStats) as Card] : []),
  ];
}

export function renderVitals(
  container: HTMLElement,
  v: ServerVitals,
  vacuumAge: VacuumClusterAge | null = null,
  checkpointer: CheckpointerStats | null = null,
  lockCapacity: LockCapacity | null = null,
  history: SnapshotHistory = { cap: 0, points: [] },
  ioStats: IoStatRow[] | null = null,
  wal: WalStats | null = null,
): void {
  container.replaceChildren(
    ...cards(v, vacuumAge, checkpointer, lockCapacity, history, ioStats, wal).map((card) => {
      const el = document.createElement("div");
      const classes = ["card", card.tone, card.lead ? "lead" : ""].filter(Boolean);
      el.className = classes.join(" ");
      const meter =
        card.meter === null
          ? ""
          : `<div class="meter"><div class="meter-fill" style="width:${(
              Math.min(1, Math.max(0, card.meter)) * 100
            ).toFixed(1)}%"></div></div>`;
      const trendSpan = card.trend
        ? ` <span class="card-trend ${card.trend.tone}" title="${escapeHtml(card.trend.title)}">${card.trend.glyph}</span>`
        : "";
      el.innerHTML = `
        <div class="card-label">${card.label}${trendSpan}</div>
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
