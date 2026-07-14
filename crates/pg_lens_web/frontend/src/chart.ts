// TPS / active-sessions time series, driven by snapshot.history (the ring
// buffer grown by the core poller — the same series the TUI sparklines use).

import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { SnapshotHistory } from "./types";

const TPS_COLOR = "#4fc3f7";
const SESSIONS_COLOR = "#ffb74d";

export class HistoryChart {
  private readonly container: HTMLElement;
  private plot: uPlot | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
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

  private size(): { width: number; height: number } {
    return { width: Math.max(280, this.container.clientWidth), height: 220 };
  }

  private resize(): void {
    this.plot?.setSize(this.size());
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
    };
  }
}
