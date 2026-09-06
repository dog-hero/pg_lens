// Progress lens (v0.17.1, Tab 8): unified in-flight maintenance & DDL operations.
//
// Pure TypeScript derivation over `DbSnapshot.ddl_progress` and
// `DbSnapshot.vacuum_progress`, mirroring the TUI's ProgressLens layout.

import type { DdlProgressRow, ProgressUnifiedRow, VacuumProgressRow } from "./types";

export interface ProgressLens {
  update(ddlProgress: DdlProgressRow[] | null, vacuumProgress: VacuumProgressRow[] | null): void;
}

export function unifyProgress(
  ddlProgress: DdlProgressRow[] | null,
  vacuumProgress: VacuumProgressRow[] | null,
): ProgressUnifiedRow[] {
  const out: ProgressUnifiedRow[] = [];

  if (ddlProgress) {
    for (const d of ddlProgress) {
      out.push({
        pid: d.pid,
        command: d.command,
        relation: d.relation,
        phase: d.phase,
        progress_pct: d.progress_pct,
        current_step: d.current_step,
        total_step: d.total_step,
        detail: d.detail,
        unit: "steps",
      });
    }
  }

  if (vacuumProgress) {
    for (const v of vacuumProgress) {
      const pct =
        v.heap_blks_total > 0
          ? (v.heap_blks_scanned / v.heap_blks_total) * 100
          : null;
      const detail =
        v.heap_blks_total > 0
          ? `heap blks: ${v.heap_blks_scanned} / ${v.heap_blks_total}`
          : "";
      out.push({
        pid: v.pid,
        command: "VACUUM",
        relation: v.relation,
        phase: v.phase,
        progress_pct: pct,
        current_step: v.heap_blks_scanned,
        total_step: v.heap_blks_total,
        detail,
        unit: "blocks",
      });
    }
  }

  out.sort((a, b) => a.pid - b.pid);
  return out;
}

export function filterProgressRows(
  rows: ProgressUnifiedRow[],
  query: string,
): ProgressUnifiedRow[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return rows;

  return rows.filter((r) => {
    return (
      String(r.pid).includes(needle) ||
      r.command.toLowerCase().includes(needle) ||
      r.relation.toLowerCase().includes(needle) ||
      r.phase.toLowerCase().includes(needle) ||
      r.detail.toLowerCase().includes(needle)
    );
  });
}

export function formatProgressGauge(pct: number | null): string {
  if (pct === null) {
    return "in progress";
  }
  return `${pct.toFixed(1)}%`;
}

export function initProgressLens(panel: HTMLElement): ProgressLens {
  const filterInput = panel.querySelector<HTMLInputElement>("#progress-filter");
  const placeholder = panel.querySelector<HTMLElement>("#progress-placeholder");
  const tbody = panel.querySelector<HTMLTableSectionElement>("#progress-table tbody");

  let latestRows: ProgressUnifiedRow[] = [];
  let filterQuery = "";

  function render(): void {
    if (!tbody || !placeholder) return;

    const filtered = filterProgressRows(latestRows, filterQuery);
    tbody.replaceChildren();

    if (filtered.length === 0) {
      placeholder.hidden = false;
      placeholder.textContent =
        latestRows.length === 0
          ? "No in-flight maintenance or DDL operations detected."
          : "No operations matching filter.";
      return;
    }

    placeholder.hidden = true;

    for (const row of filtered) {
      const tr = document.createElement("tr");

      const tdPid = document.createElement("td");
      tdPid.className = "num";
      tdPid.textContent = String(row.pid);
      tr.appendChild(tdPid);

      const tdCmd = document.createElement("td");
      const cmdBadge = document.createElement("span");
      cmdBadge.className = "meta-item meta-ddl";
      cmdBadge.textContent = row.command;
      tdCmd.appendChild(cmdBadge);
      tr.appendChild(tdCmd);

      const tdRel = document.createElement("td");
      tdRel.textContent = row.relation;
      tr.appendChild(tdRel);

      const tdPhase = document.createElement("td");
      tdPhase.textContent = row.phase;
      tr.appendChild(tdPhase);

      const tdProg = document.createElement("td");
      if (row.progress_pct !== null) {
        const wrap = document.createElement("div");
        wrap.className = "progress-bar-wrap";

        const fill = document.createElement("div");
        fill.className = "progress-bar-fill";
        const clampedPct = Math.min(100, Math.max(0, row.progress_pct));
        fill.style.width = `${clampedPct}%`;

        const label = document.createElement("span");
        label.className = "progress-bar-label";
        label.textContent = `${row.progress_pct.toFixed(1)}%`;

        wrap.appendChild(fill);
        wrap.appendChild(label);
        tdProg.appendChild(wrap);
      } else {
        const dim = document.createElement("span");
        dim.className = "dim";
        dim.textContent = "in progress";
        tdProg.appendChild(dim);
      }
      tr.appendChild(tdProg);

      const tdStep = document.createElement("td");
      tdStep.className = "num";
      const totalStr = row.total_step > 0 ? String(row.total_step) : "?";
      tdStep.textContent = `${row.current_step} / ${totalStr} ${row.unit}`;
      tr.appendChild(tdStep);

      const tdDetail = document.createElement("td");
      tdDetail.textContent = row.detail;
      tr.appendChild(tdDetail);

      tbody.appendChild(tr);
    }
  }

  if (filterInput) {
    filterInput.addEventListener("input", () => {
      filterQuery = filterInput.value;
      render();
    });
  }

  return {
    update(ddlProgress: DdlProgressRow[] | null, vacuumProgress: VacuumProgressRow[] | null): void {
      latestRows = unifyProgress(ddlProgress, vacuumProgress);
      render();
    },
  };
}
