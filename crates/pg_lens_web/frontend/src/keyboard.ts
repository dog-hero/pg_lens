// Web keyboard navigation: pure dispatch-mapping helpers, kept DOM-free so
// they're plain node:test units — main.ts's `keydown` listener is the only
// place that touches the DOM.

/** `1`–`9` jump to the 9 nav tabs, matching the TUI's 9 lenses. */
const KEY_TO_TAB_ID: Record<string, string> = {
  "1": "tab-macro",
  "2": "tab-activity",
  "3": "tab-blocks",
  "4": "tab-replication",
  "5": "tab-schema",
  "6": "tab-indexes",
  "7": "tab-queries",
  "8": "tab-progress",
  "9": "tab-records",
};

/** The tab button id a digit key jumps to, or null for any other key. */
export function tabIdForKey(key: string): string | null {
  return KEY_TO_TAB_ID[key] ?? null;
}

/** Each panel's filter input id, for `/` — panels with no filter map to null. */
const PANEL_FILTER_INPUT_ID: Record<string, string> = {
  "activity-panel": "activity-filter",
  "replication-panel": "replication-filter",
  "schema-panel": "schema-filter",
  "indexes-panel": "indexes-filter",
  "queries-panel": "statements-filter",
  "progress-panel": "progress-filter",
  "records-panel": "records-filter",
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

export interface KeyEventLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

/** `Cmd+K` or `Ctrl+K`: toggle Command Palette. */
export function isPaletteKey(event: KeyEventLike): boolean {
  if (event.altKey) return false;
  return Boolean((event.ctrlKey || event.metaKey) && (event.key === "k" || event.key === "K"));
}

/** `Shift+R` (`R`) or `Ctrl+R`: toggle incident recording mode (Flight Recorder). */
export function isRecordKey(event: KeyEventLike): boolean {
  if (event.altKey) return false;
  if (event.ctrlKey && (event.key === "r" || event.key === "R")) return true;
  return event.key === "R" && !event.ctrlKey && !event.metaKey;
}

/** `E`: export point-in-time snapshot bookmark to JSON. */
export function isExportKey(event: KeyEventLike): boolean {
  if (event.ctrlKey || event.metaKey || event.altKey) return false;
  return event.key === "E" || event.key === "e";
}

/** `B`: trigger Schema Lens recollect + bloat refresh. */
export function isSchemaRefreshKey(event: KeyEventLike): boolean {
  if (event.ctrlKey || event.metaKey || event.altKey) return false;
  return event.key === "B" || event.key === "b";
}

/** `?`: toggle Keyboard Shortcuts modal. */
export function isHelpKey(event: KeyEventLike): boolean {
  if (event.ctrlKey || event.metaKey || event.altKey) return false;
  return event.key === "?";
}
