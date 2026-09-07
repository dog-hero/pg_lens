// Records Lens (Incident Recordings & Snapshot Bookmarks)
//
// Manages offline incident recordings and bookmarks: listing, filtering,
// downloading, and deleting files via the pg_lens_web REST API.

import type { RecordingEntry } from "./types";

export interface RecordsLens {
  load(): Promise<void>;
  filter(query: string): void;
}

export function filterRecordRows(
  rows: RecordingEntry[],
  query: string,
): RecordingEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return rows;

  return rows.filter((r) => {
    return (
      r.filename.toLowerCase().includes(needle) ||
      r.target.toLowerCase().includes(needle) ||
      r.kind.toLowerCase().includes(needle) ||
      (r.started_at && r.started_at.toLowerCase().includes(needle))
    );
  });
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  const gb = mb / 1024;
  return `${gb.toFixed(2)} GB`;
}

export function formatDuration(secs: number | null): string {
  if (secs === null || secs === undefined) return "-";
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  const remM = m % 60;
  return `${h}h ${remM}m`;
}

export async function fetchRecordings(token: string | null): Promise<RecordingEntry[]> {
  const headers: Record<string, string> = {};
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  const res = await fetch("/api/records", { headers });
  if (!res.ok) {
    throw new Error(`Failed to fetch recordings: ${res.statusText}`);
  }
  return (await res.json()) as RecordingEntry[];
}

export function downloadRecording(filename: string, token: string | null): void {
  const url = token
    ? `/api/records/download/${encodeURIComponent(filename)}?token=${encodeURIComponent(token)}`
    : `/api/records/download/${encodeURIComponent(filename)}`;
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
}

export async function deleteRecording(filename: string, token: string | null): Promise<boolean> {
  const headers: Record<string, string> = {};
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  const res = await fetch(`/api/records/${encodeURIComponent(filename)}`, {
    method: "DELETE",
    headers,
  });
  return res.ok;
}

export function initRecordsLens(
  panel: HTMLElement,
  getToken: () => string | null,
  isReadOnly: () => boolean,
): RecordsLens {
  const filterInput = panel.querySelector<HTMLInputElement>("#records-filter");
  const placeholder = panel.querySelector<HTMLElement>("#records-placeholder");
  const tbody = panel.querySelector<HTMLTableSectionElement>("#records-table tbody");
  const refreshBtn = panel.querySelector<HTMLButtonElement>("#records-refresh-btn");

  let latestRecords: RecordingEntry[] = [];
  let filterQuery = "";

  function render(): void {
    if (!tbody || !placeholder) return;

    const filtered = filterRecordRows(latestRecords, filterQuery);
    tbody.replaceChildren();

    if (filtered.length === 0) {
      placeholder.hidden = false;
      const tableWrap = panel.querySelector<HTMLElement>(".table-wrap");
      if (tableWrap) tableWrap.hidden = true;
      return;
    }

    placeholder.hidden = true;
    const tableWrap = panel.querySelector<HTMLElement>(".table-wrap");
    if (tableWrap) tableWrap.hidden = false;

    for (const record of filtered) {
      const tr = document.createElement("tr");

      // Status badge
      const tdStatus = document.createElement("td");
      const badge = document.createElement("span");
      if (record.is_active) {
        badge.className = "badge badge-rec";
        badge.textContent = "● REC (ACTIVE)";
      } else if (record.kind === "Recording") {
        badge.className = "badge badge-recording";
        badge.textContent = "RECORDING";
      } else {
        badge.className = "badge badge-bookmark";
        badge.textContent = "BOOKMARK";
      }
      tdStatus.appendChild(badge);
      tr.appendChild(tdStatus);

      // Target
      const tdTarget = document.createElement("td");
      tdTarget.textContent = record.target;
      tr.appendChild(tdTarget);

      // Filename
      const tdFilename = document.createElement("td");
      tdFilename.className = "filename-cell";
      tdFilename.textContent = record.filename;
      if (record.filename.endsWith(".gz")) {
        const gzBadge = document.createElement("span");
        gzBadge.className = "badge badge-gz";
        gzBadge.textContent = "GZ";
        tdFilename.appendChild(gzBadge);
      }
      tr.appendChild(tdFilename);

      // Size
      const tdSize = document.createElement("td");
      tdSize.className = "num";
      tdSize.textContent = formatBytes(record.size_bytes);
      tr.appendChild(tdSize);

      // Frames
      const tdFrames = document.createElement("td");
      tdFrames.className = "num";
      tdFrames.textContent = record.frame_count !== null && record.frame_count !== undefined
        ? String(record.frame_count)
        : "-";
      tr.appendChild(tdFrames);

      // Started
      const tdStarted = document.createElement("td");
      tdStarted.textContent = record.started_at || "-";
      tr.appendChild(tdStarted);

      // Ended
      const tdEnded = document.createElement("td");
      tdEnded.textContent = record.ended_at || "-";
      tr.appendChild(tdEnded);

      // Duration
      const tdDuration = document.createElement("td");
      tdDuration.textContent = formatDuration(record.duration_secs);
      tr.appendChild(tdDuration);

      // Actions
      const tdActions = document.createElement("td");
      tdActions.className = "actions-cell";

      // Download btn
      const btnDownload = document.createElement("button");
      btnDownload.className = "btn-icon";
      btnDownload.title = `Download ${record.filename}`;
      btnDownload.textContent = "⬇";
      btnDownload.onclick = () => {
        downloadRecording(record.filename, getToken());
      };
      tdActions.appendChild(btnDownload);

      // Delete btn (only if not active and not read-only)
      if (!record.is_active && !isReadOnly()) {
        const btnDelete = document.createElement("button");
        btnDelete.className = "btn-icon btn-danger";
        btnDelete.title = `Delete ${record.filename}`;
        btnDelete.textContent = "🗑";
        btnDelete.onclick = async () => {
          if (confirm(`Delete recording file "${record.filename}"?`)) {
            const ok = await deleteRecording(record.filename, getToken());
            if (ok) {
              await load();
            } else {
              alert(`Failed to delete "${record.filename}"`);
            }
          }
        };
        tdActions.appendChild(btnDelete);
      }

      tr.appendChild(tdActions);
      tbody.appendChild(tr);
    }
  }

  async function load(): Promise<void> {
    try {
      latestRecords = await fetchRecordings(getToken());
      render();
    } catch (err) {
      console.error("RecordsLens fetch error:", err);
    }
  }

  if (filterInput) {
    filterInput.addEventListener("input", () => {
      filterQuery = filterInput.value;
      render();
    });
  }

  if (refreshBtn) {
    refreshBtn.addEventListener("click", () => {
      void load();
    });
  }

  return {
    load,
    filter: (query: string) => {
      filterQuery = query;
      if (filterInput) filterInput.value = query;
      render();
    },
  };
}

