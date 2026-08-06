// Demo entrypoint for the GitHub Pages build (`npm run build:demo`).
//
// The production app is imported UNMODIFIED — this module's whole job is to
// stand in for the Rust server before `main.ts` ever runs:
//
//   * `window.EventSource` is replaced by a class that replays a recorded
//     snapshot sequence (captured from `pg_lens --mock serve`) on a loop,
//     emitting `message` events whose `.data` is the same JSON string the
//     real SSE stream sends (see stream.ts's `source.onmessage`).
//   * `window.fetch` is wrapped so the handful of `/api/*` routes the app
//     calls answer from the fixtures (GET) or with a canned OK plus a
//     "demo mode" toast (POST). Everything else falls through untouched.
//
// Ordering matters: static `import` is hoisted, so main.ts is pulled in with
// a *dynamic* import after the stubs are installed.
//
// Nothing here is reachable from `index.html` / `src/main.ts`, so the bundle
// embedded in the Rust binary is byte-for-byte unaffected.

import snapshotsUrl from "../demo/snapshots.json?url";

interface Fixtures {
  /** Response body captured from `GET /api/config`. */
  config: unknown;
  /** Successive `GET /api/snapshot` bodies, ~2s apart. */
  snapshots: unknown[];
}

/** Matches the real server's SSE cadence (the default 2s poll interval). */
const TICK_MS = 2000;

const fixtures = (await fetch(snapshotsUrl).then((r) => r.json())) as Fixtures;
if (!Array.isArray(fixtures.snapshots) || fixtures.snapshots.length === 0) {
  throw new Error("pg_lens demo: fixtures contain no snapshots");
}

// Pre-serialize once: the SSE contract is a string payload, and re-stringifying
// a ~50 kB snapshot on every tick is pointless work.
const frames: string[] = fixtures.snapshots.map((s) => JSON.stringify(s));
const configBody = JSON.stringify(fixtures.config ?? { read_only: false });

let cursor = 0;
function currentFrame(): string {
  return frames[cursor % frames.length] as string;
}

// ── demo-mode feedback ────────────────────────────────────────────────────
// Reuses the app's own toast element (#toast in index.html) so the message
// looks native; falls back to a console note if the markup ever moves.
let toastTimer: number | undefined;
function demoToast(message: string): void {
  const toast = document.getElementById("toast");
  if (toast === null) {
    console.info(`pg_lens demo: ${message}`);
    return;
  }
  toast.textContent = message;
  toast.dataset["kind"] = "error";
  toast.hidden = false;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
  }, 5000);
}

// ── EventSource stub ──────────────────────────────────────────────────────

type MessageHandler = ((event: MessageEvent<string>) => void) | null;

class DemoEventSource {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;

  readonly url: string;
  readonly withCredentials = false;
  readyState = DemoEventSource.CONNECTING;

  onmessage: MessageHandler = null;
  onerror: ((event: Event) => void) | null = null;
  onopen: ((event: Event) => void) | null = null;

  #timer: number | undefined;

  constructor(url: string) {
    this.url = url;
    // First frame on the next macrotask so the caller has wired `onmessage`.
    this.#timer = window.setTimeout(() => {
      this.readyState = DemoEventSource.OPEN;
      this.onopen?.(new Event("open"));
      this.#emit();
      this.#timer = window.setInterval(() => {
        cursor = (cursor + 1) % frames.length;
        this.#emit();
      }, TICK_MS);
    }, 0);
  }

  #emit(): void {
    if (this.readyState === DemoEventSource.CLOSED) return;
    this.onmessage?.(new MessageEvent<string>("message", { data: currentFrame() }));
  }

  // The app only ever uses the `on*` properties; these exist so the stub still
  // satisfies anything that pokes at the standard surface.
  addEventListener(): void {}
  removeEventListener(): void {}
  dispatchEvent(): boolean {
    return false;
  }

  close(): void {
    this.readyState = DemoEventSource.CLOSED;
    window.clearTimeout(this.#timer);
    window.clearInterval(this.#timer);
  }
}

// ── fetch stub ────────────────────────────────────────────────────────────

function json(body: string): Response {
  return new Response(body, {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

/** POST routes the app fires: all are no-ops here, answered with a canned OK
 * plus a toast explaining why nothing changed. The toast is deferred a tick
 * so it lands *after* whatever success message main.ts writes on resolve. */
const POST_NOTICE: Array<[string, string]> = [
  ["/api/db/switch", "Demo mode — no live database, so the target cannot be switched"],
  ["/api/schema/refresh", "Demo mode — no live database, so there is nothing to re-collect"],
  ["/api/admin/", "Demo mode — no live database, so no backend was signalled"],
];

const realFetch = window.fetch.bind(window);

function pathOf(input: RequestInfo | URL): string {
  const raw =
    typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  try {
    return new URL(raw, window.location.href).pathname;
  } catch {
    return raw;
  }
}

window.fetch = ((input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const path = pathOf(input);
  if (!path.startsWith("/api/")) return realFetch(input as RequestInfo, init);

  if (path === "/api/snapshot") return Promise.resolve(json(currentFrame()));
  if (path === "/api/config") return Promise.resolve(json(configBody));

  // `POST /api/schema/detail` is a queue-only request in production too — the
  // structure payload rides the snapshot stream as `table_detail`, which the
  // fixtures already carry, so a bare OK is the faithful answer.
  if (path === "/api/schema/detail") return Promise.resolve(json("{}"));

  const notice = POST_NOTICE.find(([prefix]) => path.startsWith(prefix))?.[1];
  if (notice !== undefined) {
    window.setTimeout(() => demoToast(notice), 30);
    return Promise.resolve(json("{}"));
  }
  return Promise.resolve(json("{}"));
}) as typeof window.fetch;

window.EventSource = DemoEventSource as unknown as typeof EventSource;

// Now — and only now — boot the real application.
await import("./main.ts");
