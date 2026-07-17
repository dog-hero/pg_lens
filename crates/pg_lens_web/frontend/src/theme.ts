// Light/dark theme (v0.13 redesign): the palette itself lives in
// style.css's two `:root[data-theme=...]` variable sets — this module only
// holds the pure decision logic (what theme to boot with, what "toggle"
// means, where it persists) so it's testable without a DOM/browser.
//
// Default is dark: it's the existing behavior (every screenshot, every demo,
// the TUI itself is a dark terminal by definition), so an upgrade must not
// silently repaint returning users' screens. `prefers-color-scheme` is
// deliberately NOT consulted for the default — an ops dashboard being dark
// by default regardless of OS setting is a reasonable, unsurprising choice,
// and it keeps the default trivially testable (one code path, not an
// environment-dependent one). Explicit opt-in (the toggle) always wins and
// is remembered.

export type Theme = "dark" | "light";

export const THEME_STORAGE_KEY = "pg_lens_theme";

/** Boot-time theme: whatever was persisted, defaulting to dark for anything
 * else (missing key, corrupted value, first visit). */
export function resolveInitialTheme(stored: string | null): Theme {
  return stored === "light" ? "light" : "dark";
}

/** The toggle button always just flips — there is no third state. */
export function nextTheme(current: Theme): Theme {
  return current === "dark" ? "light" : "dark";
}

/** Best-effort localStorage read: private-browsing / disabled storage must
 * never throw and break boot. */
export function loadStoredTheme(storage: Storage): string | null {
  try {
    return storage.getItem(THEME_STORAGE_KEY);
  } catch {
    return null;
  }
}

/** Best-effort localStorage write, same failure contract as the loader. */
export function saveTheme(storage: Storage, theme: Theme): void {
  try {
    storage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Storage unavailable (private mode, quota) — the toggle still works
    // for the rest of the session, it just won't persist. Not worth surfacing.
  }
}
