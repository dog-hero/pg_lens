// Inline SVG icon set (v0.13 redesign): no icon font, no icon library — the
// actual `<symbol>` markup lives once in index.html's sprite (`<svg
// id="icon-sprite">`), referenced everywhere via `<use href="#icon-...">`.
// This module only holds the pure name→symbol-id lookup so the mapping is
// unit-testable and callers (main.ts, and index.html's hand-written buttons)
// share one source of truth for the id strings instead of duplicating them.

/** The nine nav sections, matching the TUI's 9 lenses. */
export type NavSection =
  | "macro"
  | "activity"
  | "blocks"
  | "replication"
  | "schema"
  | "indexes"
  | "queries"
  | "progress"
  | "records";

const NAV_ICON_IDS: Record<NavSection, string> = {
  macro: "icon-macro",
  activity: "icon-activity",
  blocks: "icon-blocks",
  replication: "icon-replication",
  schema: "icon-schema",
  indexes: "icon-indexes",
  queries: "icon-queries",
  progress: "icon-progress",
  records: "icon-records",
};

/** `<symbol>` id for a nav section's icon (index.html's sprite defines it). */
export function navIconId(section: NavSection): string {
  return NAV_ICON_IDS[section];
}

/** Severity → symbol id, for the small legend and any inline status glyphs
 * (kept in sync with the CSS severity tokens: ok/warn/bad/info). */
export type Severity = "ok" | "warn" | "bad" | "info";

const SEVERITY_ICON_IDS: Record<Severity, string> = {
  ok: "icon-dot",
  warn: "icon-dot",
  bad: "icon-dot",
  info: "icon-dot",
};

export function severityIconId(severity: Severity): string {
  return SEVERITY_ICON_IDS[severity];
}
