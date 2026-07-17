// Blocking chain / lock-wait graph (v0.9).
//
// TypeScript port of pg_lens_core/src/blocking.rs (`blocking_chain`) — same
// walk, same cycle detection, kept in lockstep the way waits.ts mirrors
// waits.rs. Pure derivation over `DbSnapshot.locks`, no new fetch.

import type { LockRow } from "./types";

export interface BlockingChain {
  /** chain[0] is the pid this was built for; chain[last] is the root
   * blocker (or, on a deadlock, the repeated pid that closes the cycle). */
  chain: number[];
  /** True when the walk revisited a pid already in `chain` — a genuine
   * wait-for cycle rather than terminating at a free session. */
  deadlock: boolean;
}

/**
 * Build the wait-for chain for `pid`, or `null` when `pid` is not itself
 * blocked (no matching `LockRow`). Each blocked backend can report several
 * blockers (`blocked_by` is an array) — the chain follows the FIRST one at
 * every step, same simplification as the Rust core.
 *
 * Bounded by construction: `visited` is checked before every step extends
 * the chain, so a cycle stops the walk on its first repeat — never an
 * infinite loop.
 */
export function blockingChain(pid: number, locks: LockRow[]): BlockingChain | null {
  const byPid = new Map<number, LockRow>();
  for (const lock of locks) byPid.set(lock.pid, lock);
  if (!byPid.has(pid)) return null;

  const chain = [pid];
  const visited = new Set<number>([pid]);
  let current = pid;
  let deadlock = false;

  for (;;) {
    const row = byPid.get(current);
    const next = row?.blocked_by[0];
    if (row === undefined || next === undefined) break; // root: not blocked itself.
    if (visited.has(next)) {
      chain.push(next);
      deadlock = true;
      break;
    }
    chain.push(next);
    visited.add(next);
    current = next;
  }

  return { chain, deadlock };
}

/**
 * Render one wait-for chain as a `<span>` element: pids joined by arrows,
 * the root blocker (last link) highlighted red/bold, a deadlock cycle
 * flagged with an explicit warning line — mirrors the TUI detail panel.
 */
export function renderBlockingChain(chain: BlockingChain): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "blocking-chain";

  const line = document.createElement("div");
  line.className = "blocking-chain-line";
  chain.chain.forEach((pid, i) => {
    if (i > 0) {
      const arrow = document.createElement("span");
      arrow.className = "blocking-chain-arrow";
      arrow.textContent = " \u{2192} ";
      line.append(arrow);
    }
    const isRoot = i === chain.chain.length - 1;
    const span = document.createElement("span");
    span.textContent = String(pid);
    if (isRoot) {
      span.className = "blocking-chain-root";
      span.title = chain.deadlock ? "part of a deadlock cycle" : "root blocker — act on this one";
    }
    line.append(span);
  });
  wrap.append(line);

  if (chain.deadlock) {
    const warn = document.createElement("div");
    warn.className = "blocking-chain-deadlock";
    warn.textContent = "⚠ deadlock cycle detected — the chain loops back on itself";
    wrap.append(warn);
  }

  return wrap;
}
