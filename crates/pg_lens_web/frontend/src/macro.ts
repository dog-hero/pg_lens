// Macro Lens: cluster-wide vitals, history chart, and saturation metrics
// Mirrors the TUI's Lens 1 (MacroLens).

import type {
  CheckpointerStats,
  IoStatRow,
  LockCapacity,
  ServerVitals,
  SlruStats,
  SnapshotHistory,
  VacuumClusterAge,
  WalStats,
} from "./types.ts";
import { renderVitals } from "./vitals.ts";
import { HistoryChart } from "./chart.ts";
import {
  cacheHitReadoutSeverity,
  formatPinAge,
  formatReadoutTime,
  lockPressureReadoutSeverity,
  readoutAtIndex,
  resolvePinnedIndex,
  type ReadoutPoint,
} from "./scrubber.ts";

export class MacroLens {
  private vitalsContainer: HTMLElement;
  private chartContainer: HTMLElement;
  private scrubReadout: HTMLElement;
  private scrubReadoutTime: HTMLElement;
  private scrubReadoutPinnedHint: HTMLElement;
  private scrubReadoutTps: HTMLElement;
  private scrubReadoutSessions: HTMLElement;
  private scrubReadoutConns: HTMLElement;
  private scrubReadoutCache: HTMLElement;
  private scrubReadoutLock: HTMLElement;
  private scrubReadoutXidWrap: HTMLElement;
  private scrubReadoutXid: HTMLElement;
  private scrubUnpinBtn: HTMLButtonElement;

  private chart: HistoryChart;
  private currentHistory: SnapshotHistory = { cap: 0, points: [] };
  private pinnedEpochMs: number | null = null;

  constructor(panel: HTMLElement) {
    this.vitalsContainer = panel.querySelector("#vitals") as HTMLElement;
    this.chartContainer = panel.querySelector("#chart") as HTMLElement;
    this.scrubReadout = panel.querySelector("#scrub-readout") as HTMLElement;
    this.scrubReadoutTime = panel.querySelector("#scrub-readout-time") as HTMLElement;
    this.scrubReadoutPinnedHint = panel.querySelector("#scrub-readout-pinned-hint") as HTMLElement;
    this.scrubReadoutTps = panel.querySelector("#scrub-readout-tps") as HTMLElement;
    this.scrubReadoutSessions = panel.querySelector("#scrub-readout-sessions") as HTMLElement;
    this.scrubReadoutConns = panel.querySelector("#scrub-readout-conns") as HTMLElement;
    this.scrubReadoutCache = panel.querySelector("#scrub-readout-cache") as HTMLElement;
    this.scrubReadoutLock = panel.querySelector("#scrub-readout-lock") as HTMLElement;
    this.scrubReadoutXidWrap = panel.querySelector("#scrub-readout-xid-wrap") as HTMLElement;
    this.scrubReadoutXid = panel.querySelector("#scrub-readout-xid") as HTMLElement;
    this.scrubUnpinBtn = panel.querySelector("#scrub-unpin") as HTMLButtonElement;

    this.chart = new HistoryChart(this.chartContainer, {
      onHover: (idx) => {
        if (this.pinnedEpochMs !== null) return;
        const r = readoutAtIndex(this.currentHistory, idx);
        if (r === null) this.hideReadout();
        else this.renderReadout(r, false);
      },
      onClick: (idx) => {
        if (this.pinnedEpochMs !== null) {
          this.unpinScrub();
          return;
        }
        const r = readoutAtIndex(this.currentHistory, idx);
        if (r !== null) this.pinScrub(r);
      },
    });

    this.scrubUnpinBtn?.addEventListener("click", () => this.unpinScrub());
  }

  update(
    v: ServerVitals,
    vacuumAge: VacuumClusterAge | null = null,
    checkpointer: CheckpointerStats | null = null,
    lockCapacity: LockCapacity | null = null,
    history: SnapshotHistory = { cap: 0, points: [] },
    ioStats: IoStatRow[] | null = null,
    wal: WalStats | null = null,
    slru: SlruStats | null = null,
  ): void {
    this.currentHistory = history;
    renderVitals(
      this.vitalsContainer,
      v,
      vacuumAge,
      checkpointer,
      lockCapacity,
      history,
      ioStats,
      wal,
      slru,
    );
    this.chart.update(history);

    if (this.pinnedEpochMs !== null) {
      const idx = resolvePinnedIndex(history, this.pinnedEpochMs);
      if (idx === null) {
        this.unpinScrub();
      } else {
        const r = readoutAtIndex(history, idx);
        if (r !== null) this.renderReadout(r, true);
        this.chart.setPinMarker(this.pinnedEpochMs / 1000);
      }
    }
  }

  isPinned(): boolean {
    return this.pinnedEpochMs !== null;
  }

  unpinScrub(): void {
    this.pinnedEpochMs = null;
    this.chart.setPinMarker(null);
    this.hideReadout();
  }

  stepPin(direction: -1 | 1): void {
    if (this.pinnedEpochMs === null || this.currentHistory.points.length === 0) return;
    const idx = resolvePinnedIndex(this.currentHistory, this.pinnedEpochMs);
    if (idx === null) return;
    const nextIdx = Math.max(0, Math.min(this.currentHistory.points.length - 1, idx + direction));
    const r = readoutAtIndex(this.currentHistory, nextIdx);
    if (r !== null) {
      this.pinScrub(r);
    }
  }

  private pinScrub(r: ReadoutPoint): void {
    this.pinnedEpochMs = r.epochMs;
    this.chart.setPinMarker(r.epochMs / 1000);
    this.renderReadout(r, true);
  }

  private renderReadout(r: ReadoutPoint, pinned: boolean): void {
    this.scrubReadoutTime.textContent = formatReadoutTime(r.epochMs);
    if (pinned) {
      this.scrubReadoutPinnedHint.textContent = `(pinned ${formatPinAge(r.epochMs, Date.now())} · Esc unpins)`;
      this.scrubReadoutPinnedHint.hidden = false;
      this.scrubUnpinBtn.hidden = false;
    } else {
      this.scrubReadoutPinnedHint.hidden = true;
      this.scrubUnpinBtn.hidden = true;
    }
    this.scrubReadoutTps.textContent = r.tps.toFixed(1);
    this.scrubReadoutSessions.textContent = String(r.activeSessions);
    this.scrubReadoutConns.textContent = String(r.connectionsTotal);

    if (r.cacheHitPct !== null) {
      this.scrubReadoutCache.textContent = `${r.cacheHitPct.toFixed(1)}%`;
      const sev = cacheHitReadoutSeverity(r.cacheHitPct);
      this.scrubReadoutCache.className = sev;
    } else {
      this.scrubReadoutCache.textContent = "—";
      this.scrubReadoutCache.className = "";
    }

    if (r.lockPressurePct !== null) {
      this.scrubReadoutLock.textContent = `${r.lockPressurePct.toFixed(1)}%`;
      const sev = lockPressureReadoutSeverity(r.lockPressurePct);
      this.scrubReadoutLock.className = sev;
    } else {
      this.scrubReadoutLock.textContent = "—";
      this.scrubReadoutLock.className = "";
    }

    if (r.oldestXidAge !== null) {
      this.scrubReadoutXid.textContent = r.oldestXidAge.toLocaleString();
      this.scrubReadoutXidWrap.hidden = false;
    } else {
      this.scrubReadoutXidWrap.hidden = true;
    }

    this.scrubReadout.hidden = false;
  }

  private hideReadout(): void {
    this.scrubReadout.hidden = true;
  }
}
