// Copy-to-clipboard for the expanded query/statement detail rows (v0.16,
// Part C's web parity — the TUI twin uses OSC 52; the browser has a real
// clipboard API instead).
//
// `navigator.clipboard.writeText` needs a secure context (HTTPS, or
// localhost) — `copyText` falls back to the classic `document.execCommand
// ("copy")` dance (a hidden, focused, selected `<textarea>`) for older
// browsers or an insecure (plain HTTP, non-localhost) deployment, so the
// button still works there. No new dependency either way — both mechanisms
// are already built into the DOM/browser API surface.

/** Copies `text` to the clipboard, trying the modern async API first and
 * falling back to the `execCommand` trick. Resolves `true` only if a
 * mechanism reports success — callers must not assume a copy happened just
 * because this was called (see `renderCopyButton`'s honest toast wording). */
export async function copyText(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Fall through to the legacy path — a permission prompt dismissal or
      // an unsupported API surface both land here.
    }
  }
  return legacyCopy(text);
}

/** `document.execCommand("copy")` via a hidden, off-screen, selected
 * `<textarea>` — works without a secure context, the only reason this
 * fallback exists at all (the API itself is deprecated but still broadly
 * implemented for exactly this reason). */
function legacyCopy(text: string): boolean {
  if (typeof document === "undefined") return false;
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "-1000px";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.focus();
  textarea.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  textarea.remove();
  return ok;
}

/** Builds a small "Copy" button that copies `getText()`'s result on click
 * and reports the outcome (success + character count, so callers can render
 * an honest "copied N chars" / "copy failed — select the text manually"
 * toast — never a bare unconditional "copied!"). Stops the click from
 * bubbling so it never toggles a parent row's own expand/collapse handler. */
export function renderCopyButton(
  getText: () => string,
  onResult: (ok: boolean, chars: number) => void,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "Copy";
  button.classList.add("copy-btn");
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    const text = getText();
    void copyText(text).then((ok) => onResult(ok, text.length));
  });
  return button;
}
