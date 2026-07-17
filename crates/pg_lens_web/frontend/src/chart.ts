// TPS / active-sessions time series, driven by snapshot.history (the ring
// buffer grown by the core poller — the same series the TUI sparklines use).

import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { SnapshotHistory } from "./types";

const TPS_COLOR = "#4fc3f7";
const SESSIONS_COLOR = "#ffb74d";
/** Pinned-moment marker line — matches the accent used elsewhere for
 * "you're looking at something specific" (the schema staleness dot, etc). */
const PIN_COLOR = "#4fc3f7";

/** Wired by main.ts's scrubber: fired on every cursor move (hover) and on a
 * click inside the plotting area, both as data-array indices (uPlot's own
 * `cursor.idx` — `null` on hover means "pointer left the plot"). */
export interface ChartCallbacks {
  onHover: (idx: number | null) => void;
  onClick: (idx: number) => void;
}

export class HistoryChart {
  private readonly container: HTMLElement;
  private readonly callbacks: ChartCallbacks | undefined;
  private plot: uPlot | null = null;
  /** x-scale value (epoch seconds, matching the series' own x unit) of the
   * pinned moment; `null` draws nothing. Survives `update()` because the
   * draw hook re-reads it on every redraw instead of being baked into data. */
  private pinnedXVal: number | null = null;

  constructor(container: HTMLElement, callbacks?: ChartCallbacks) {
    this.container = container;
    this.callbacks = callbacks;
    window.addEventListener("resize", () => this.resize());
  }

  update(history: SnapshotHistory): void {
    const xs = history.points.map((p) => p.epoch_ms / 1000);
    const tps = history.points.map((p) => p.tps);
    const sessions = history.points.map((p) => p.active_sessions);
    const data: uPlot.AlignedData = [xs, tps, sessions];
    if (this.plot === null) {
      this.plot = new uPlot(this.options(), data, this.container);
    } else {
      this.plot.setData(data);
    }
  }

  /** Shows (or, with `null`, hides) the pinned-moment marker line at the
   * given epoch-seconds x-value — called by main.ts's scrubber wiring. */
  setPinMarker(epochSecs: number | null): void {
    this.pinnedXVal = epochSecs;
    this.plot?.redraw(false, false);
  }

  private size(): { width: number; height: number } {
    return { width: Math.max(280, this.container.clientWidth), height: 220 };
  }

  private resize(): void {
    this.plot?.setSize(this.size());
  }

  private drawPinMarker(u: uPlot): void {
    if (this.pinnedXVal === null) return;
    const x = u.valToPos(this.pinnedXVal, "x", true);
    if (x < u.bbox.left || x > u.bbox.left + u.bbox.width) return;
    const ctx = u.ctx;
    ctx.save();
    ctx.strokeStyle = PIN_COLOR;
    ctx.lineWidth = 2;
    ctx.setLineDash([4, 3]);
    ctx.beginPath();
    ctx.moveTo(x, u.bbox.top);
    ctx.lineTo(x, u.bbox.top + u.bbox.height);
    ctx.stroke();
    ctx.restore();
  }

  private options(): uPlot.Options {
    const axisStyle: uPlot.Axis = {
      stroke: "#8b949e",
      grid: { stroke: "rgba(139, 148, 158, 0.12)" },
      ticks: { stroke: "rgba(139, 148, 158, 0.25)" },
    };
    return {
      ...this.size(),
      series: [
        {},
        {
          label: "TPS",
          scale: "tps",
          stroke: TPS_COLOR,
          width: 2,
          fill: "rgba(79, 195, 247, 0.08)",
        },
        {
          label: "Active sessions",
          scale: "sessions",
          stroke: SESSIONS_COLOR,
          width: 2,
        },
      ],
      axes: [
        { ...axisStyle },
        { ...axisStyle, scale: "tps", label: "TPS" },
        {
          ...axisStyle,
          scale: "sessions",
          side: 1,
          label: "sessions",
          grid: { show: false },
        },
      ],
      scales: {
        tps: { range: (_u, _min, max) => [0, Math.max(1, max)] },
        sessions: { range: (_u, _min, max) => [0, Math.max(1, max)] },
      },
      legend: { live: true },
      cursor: { drag: { setScale: false } },
      hooks: {
        setCursor: [
          (u) => {
            this.callbacks?.onHover(u.cursor.idx ?? null);
          },
        ],
        draw: [(u) => this.drawPinMarker(u)],
        ready: [
          (u) => {
            // uPlot has no built-in "click" hook — `u.over` is the
            // pointer-event-owning overlay div documented for exactly this.
            u.over.addEventListener("click", () => {
              const idx = u.cursor.idx;
              if (idx !== null && idx !== undefined) this.callbacks?.onClick(idx);
            });
          },
        ],
      },
    };
  }
}
