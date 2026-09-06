// Blocks & Locks lens (v0.17): wait-for dependency tree and active locks table.
//
// Pure TypeScript rendering over `DbSnapshot.blocking_tree` and
// `DbSnapshot.active_locks`, mirroring the TUI's BlocksLens layout.

import type { ActiveLockRow, BlockTreeNode } from "./types";
import { humanDuration } from "./format.ts";

export interface BlocksLens {
  update(tree: BlockTreeNode[] | null, locks: ActiveLockRow[] | null): void;
}

export function lockStatusBadge(granted: boolean): { text: string; className: string } {
  return granted
    ? { text: "GRANT", className: "badge badge-ok" }
    : { text: "WAIT", className: "badge badge-bad" };
}

export function blockNodeBadge(node: BlockTreeNode): { text: string; className: string } | null {
  if (node.is_deadlock) {
    return { text: "DEADLOCK", className: "badge badge-bad" };
  }
  if (node.is_root) {
    const s = node.num_descendants === 1 ? "" : "s";
    return {
      text: `ROOT BLOCKER (${node.num_descendants} waiter${s})`,
      className: "badge badge-bad",
    };
  }
  return null;
}

export function countTreeNodes(nodes: BlockTreeNode[]): number {
  let count = 0;
  for (const n of nodes) {
    count += 1 + countTreeNodes(n.children || []);
  }
  return count;
}

export function initBlocksLens(panel: HTMLElement): BlocksLens {
  const treeContainer = panel.querySelector<HTMLElement>("#blocks-tree-container");
  const locksTbody = panel.querySelector<HTMLTableSectionElement>("#blocks-locks tbody");

  return {
    update(tree: BlockTreeNode[] | null, locks: ActiveLockRow[] | null): void {
      if (treeContainer) {
        renderTree(treeContainer, tree);
      }
      if (locksTbody) {
        renderLocks(locksTbody, locks);
      }
    },
  };
}

export function renderTree(container: HTMLElement, tree: BlockTreeNode[] | null): void {
  container.replaceChildren();

  if (!tree || tree.length === 0) {
    const empty = document.createElement("p");
    empty.className = "placeholder";
    empty.textContent = "No blocked sessions detected — all queries running freely.";
    container.appendChild(empty);
    return;
  }

  const list = document.createElement("div");
  list.className = "blocks-tree-list";

  function renderNodes(nodes: BlockTreeNode[], depth: number): void {
    for (const node of nodes) {
      const row = document.createElement("div");
      row.className = "blocks-tree-row";
      row.style.setProperty("--depth", String(depth));

      const header = document.createElement("div");
      header.className = "blocks-tree-header";

      const pidSpan = document.createElement("span");
      pidSpan.className = "blocks-tree-pid";
      pidSpan.textContent = `PID ${node.pid}`;
      header.appendChild(pidSpan);

      const badgeInfo = blockNodeBadge(node);
      if (badgeInfo) {
        const badge = document.createElement("span");
        badge.className = badgeInfo.className;
        badge.textContent = badgeInfo.text;
        header.appendChild(badge);
      }

      const userApp = document.createElement("span");
      userApp.className = "blocks-tree-user";
      userApp.textContent = `${node.usename}@${node.application_name}`;
      header.appendChild(userApp);

      const state = document.createElement("span");
      state.className = `blocks-tree-state ${node.state === "idle in transaction" ? "warn" : ""}`;
      state.textContent = node.state;
      header.appendChild(state);

      if (node.mode) {
        const lockInfo = document.createElement("span");
        lockInfo.className = "blocks-tree-lock";
        lockInfo.textContent = node.relation ? `${node.mode} on ${node.relation}` : node.mode;
        header.appendChild(lockInfo);
      }

      const dur = document.createElement("span");
      dur.className = "blocks-tree-dur";
      dur.textContent = humanDuration(node.duration_secs);
      header.appendChild(dur);

      row.appendChild(header);

      if (node.query) {
        const q = document.createElement("div");
        q.className = "blocks-tree-query sql-text";
        q.textContent = node.query;
        row.appendChild(q);
      }

      list.appendChild(row);

      if (node.children && node.children.length > 0) {
        renderNodes(node.children, depth + 1);
      }
    }
  }

  renderNodes(tree, 0);
  container.appendChild(list);
}

export function renderLocks(tbody: HTMLTableSectionElement, locks: ActiveLockRow[] | null): void {
  tbody.replaceChildren();

  if (!locks || locks.length === 0) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 8;
    td.className = "placeholder";
    td.textContent = "No active relation or transaction locks in database.";
    tr.appendChild(td);
    tbody.appendChild(tr);
    return;
  }

  for (const lock of locks) {
    const tr = document.createElement("tr");
    if (!lock.granted) {
      tr.className = "lock-waiting-row";
    }

    const tdPid = document.createElement("td");
    tdPid.textContent = String(lock.pid);
    tdPid.className = "num";

    const tdType = document.createElement("td");
    tdType.textContent = lock.locktype;

    const tdRel = document.createElement("td");
    tdRel.textContent = lock.relation
      ? (lock.schema && lock.schema !== "public" ? `${lock.schema}.${lock.relation}` : lock.relation)
      : "—";

    const tdMode = document.createElement("td");
    tdMode.textContent = lock.mode;

    const tdStatus = document.createElement("td");
    const badgeInfo = lockStatusBadge(lock.granted);
    const statusBadge = document.createElement("span");
    statusBadge.className = badgeInfo.className;
    statusBadge.textContent = badgeInfo.text;
    tdStatus.appendChild(statusBadge);

    const tdAge = document.createElement("td");
    tdAge.textContent = humanDuration(lock.duration_secs);
    tdAge.className = "num";

    const tdUser = document.createElement("td");
    tdUser.textContent = lock.usename || "—";

    const tdQuery = document.createElement("td");
    tdQuery.className = "query-cell sql-text";
    tdQuery.textContent = lock.query.replace(/\n/g, " ");

    tr.append(tdPid, tdType, tdRel, tdMode, tdStatus, tdAge, tdUser, tdQuery);
    tbody.appendChild(tr);
  }
}
