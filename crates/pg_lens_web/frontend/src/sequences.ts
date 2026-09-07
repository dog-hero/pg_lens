// Sequence exhaustion monitoring (pg_sequences, v0.19).
// Displays sequences nearing integer overflow with severity tiers (warn >= 75%, crit >= 90%).

import type { SequenceRow, SequenceSeverity } from "./types.ts";
import { humanCount } from "./format.ts";

export function sequenceMarker(sev: SequenceSeverity): string {
  switch (sev) {
    case "Critical":
      return "!!";
    case "Warning":
      return "!";
    default:
      return "";
  }
}

export function sequenceSeverityClass(sev: SequenceSeverity): string {
  switch (sev) {
    case "Critical":
      return "seq-crit";
    case "Warning":
      return "seq-warn";
    default:
      return "";
  }
}

export function formatPercentUsed(pct: number | null): string {
  return pct !== null ? `${pct.toFixed(1)}%` : "—";
}

export function formatRemaining(rem: number | null): string {
  return rem !== null ? humanCount(rem) : "—";
}

export function formatCurrent(val: number | null): string {
  return val !== null ? humanCount(val) : "—";
}

export function sequencesSummaryText(sequences: SequenceRow[]): string {
  const crit = sequences.filter((s) => s.severity === "Critical").length;
  const warn = sequences.filter((s) => s.severity === "Warning").length;
  const count = sequences.length;
  const base = `${count} sequence${count === 1 ? "" : "s"}`;
  if (crit > 0 || warn > 0) {
    return `${base} · ${crit} critical · ${warn} warning`;
  }
  return `${base} · all healthy`;
}

function severityRank(sev: SequenceSeverity): number {
  switch (sev) {
    case "Critical":
      return 0;
    case "Warning":
      return 1;
    default:
      return 2;
  }
}

export class SequencesPanel {
  private readonly thead: HTMLTableSectionElement;
  private readonly tbody: HTMLTableSectionElement;
  private readonly placeholder: HTMLElement;
  private readonly staleness: HTMLElement | null;

  constructor(
    table: HTMLTableElement,
    placeholder: HTMLElement,
    staleness?: HTMLElement | null,
  ) {
    this.placeholder = placeholder;
    this.staleness = staleness ?? null;
    this.thead = table.tHead ?? table.createTHead();
    this.tbody = table.tBodies[0] ?? table.createTBody();
    this.renderHead();
  }

  private renderHead(): void {
    const tr = document.createElement("tr");
    const cols: Array<[string, boolean]> = [
      ["!", false],
      ["Sequence", false],
      ["Type", false],
      ["Current", true],
      ["Max", true],
      ["Remaining", true],
      ["% Used", true],
    ];
    for (const [label, num] of cols) {
      const th = document.createElement("th");
      th.textContent = label;
      if (num) th.classList.add("num");
      tr.append(th);
    }
    this.thead.replaceChildren(tr);
  }

  update(sequences: SequenceRow[] | null | undefined): void {
    if (!sequences || sequences.length === 0) {
      this.placeholder.hidden = false;
      this.tbody.replaceChildren();
      if (this.staleness) this.staleness.textContent = "";
      return;
    }

    this.placeholder.hidden = true;
    if (this.staleness) {
      this.staleness.textContent = sequencesSummaryText(sequences);
    }

    const sorted = [...sequences].sort((a, b) => {
      const rDiff = severityRank(a.severity) - severityRank(b.severity);
      if (rDiff !== 0) return rDiff;
      const aPct = a.percent_used ?? -1;
      const bPct = b.percent_used ?? -1;
      if (bPct !== aPct) return bPct - aPct;
      const ka = `${a.schema}.${a.sequence_name}`;
      const kb = `${b.schema}.${b.sequence_name}`;
      return ka.localeCompare(kb);
    });

    const rows: HTMLTableRowElement[] = [];
    for (const seq of sorted) {
      const tr = document.createElement("tr");
      const cls = sequenceSeverityClass(seq.severity);
      if (cls) tr.classList.add(cls);

      const cells: Array<[string, boolean]> = [
        [sequenceMarker(seq.severity), false],
        [`${seq.schema}.${seq.sequence_name}`, false],
        [seq.data_type, false],
        [formatCurrent(seq.last_value), true],
        [humanCount(seq.max_value), true],
        [formatRemaining(seq.remaining_count), true],
        [formatPercentUsed(seq.percent_used), true],
      ];

      for (const [text, num] of cells) {
        const td = document.createElement("td");
        td.textContent = text;
        if (num) td.classList.add("num");
        tr.append(td);
      }
      rows.push(tr);
    }

    this.tbody.replaceChildren(...rows);
  }
}
