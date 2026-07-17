// Web keyboard navigation (v0.13/ROADMAP "Web keyboard navigation"): pure
// dispatch-mapping helpers, kept DOM-free so they're plain node:test units —
// main.ts's `keydown` listener is the only place that touches the DOM.

/** `1`–`5` jump to the nav tabs, same order as the sidenav / #tabs buttons. */
const KEY_TO_TAB_ID: Record<string, string> = {
  "1": "tab-activity",
  "2": "tab-replication",
  "3": "tab-schema",
  "4": "tab-indexes",
  "5": "tab-queries",
};

/** The tab button id a digit key jumps to, or null for any other key. */
export function tabIdForKey(key: string): string | null {
  return KEY_TO_TAB_ID[key] ?? null;
}

/** Each panel's filter input id, for `/` — panels with no filter (Replication,
 * Indexes) map to null and the key is a no-op there. */
const PANEL_FILTER_INPUT_ID: Record<string, string> = {
  "activity-panel": "activity-filter",
  "schema-panel": "schema-filter",
  "queries-panel": "statements-filter",
};

/** The filter `<input>` id for the currently visible panel, or null if that
 * panel has no filter. */
export function filterInputIdForPanel(panelId: string): string | null {
  return PANEL_FILTER_INPUT_ID[panelId] ?? null;
}

/** Tag names that already consume keystrokes as text — shortcuts must not
 * fire while one of these is focused (except Escape, handled separately). */
const EDITABLE_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);

export function isEditableTag(tagName: string): boolean {
  return EDITABLE_TAGS.has(tagName);
}
