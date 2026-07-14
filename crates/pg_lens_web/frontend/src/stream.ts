// SSE plumbing with optional bearer-token auth.
//
// EventSource cannot set an `Authorization` header, so when the server has
// PG_LENS_AUTH_TOKEN configured the stream authenticates with `?token=`
// instead — the server accepts the query parameter as an equivalent of the
// header (same constant-time comparison). Trade-off documented server-side
// and in the README: the token can end up in access logs / proxy logs, so
// treat it as revocable and keep TLS in front of any remote deploy.

import type { DbSnapshot } from "./types";

const TOKEN_KEY = "pg_lens_token";

export function storedToken(): string | null {
  return sessionStorage.getItem(TOKEN_KEY);
}

export function storeToken(token: string): void {
  sessionStorage.setItem(TOKEN_KEY, token);
}

export function clearToken(): void {
  sessionStorage.removeItem(TOKEN_KEY);
}

function streamUrl(token: string | null): string {
  return token === null
    ? "/api/stream"
    : `/api/stream?token=${encodeURIComponent(token)}`;
}

/**
 * Probe /api/snapshot to distinguish "server needs a token" (401) from
 * "server unreachable". Uses the Authorization header (fetch can), keeping
 * the token out of this request's URL.
 */
export async function probeAuth(
  token: string | null,
): Promise<"ok" | "unauthorized" | "unreachable"> {
  try {
    const headers: HeadersInit =
      token === null ? {} : { Authorization: `Bearer ${token}` };
    const res = await fetch("/api/snapshot", { headers });
    if (res.status === 401) return "unauthorized";
    return res.ok ? "ok" : "unreachable";
  } catch {
    return "unreachable";
  }
}

export interface StreamHandlers {
  onSnapshot: (snapshot: DbSnapshot) => void;
  onStateChange: (state: "live" | "reconnecting") => void;
  /** Server rejected our credentials — caller should prompt for a token. */
  onUnauthorized: () => void;
}

export interface StreamHandle {
  close(): void;
}

/**
 * Open the SSE stream. EventSource auto-reconnects on network errors; on
 * error we additionally probe once so an invalid/revoked token surfaces as
 * a token prompt instead of an infinite silent retry loop.
 */
export function openStream(
  token: string | null,
  handlers: StreamHandlers,
): StreamHandle {
  const source = new EventSource(streamUrl(token));
  let probing = false;

  source.onmessage = (event: MessageEvent<string>) => {
    handlers.onStateChange("live");
    try {
      handlers.onSnapshot(JSON.parse(event.data) as DbSnapshot);
    } catch (error) {
      console.error("pg_lens: bad snapshot payload", error);
    }
  };

  source.onerror = () => {
    handlers.onStateChange("reconnecting");
    if (probing) return;
    probing = true;
    void probeAuth(token).then((verdict) => {
      probing = false;
      if (verdict === "unauthorized") {
        source.close();
        handlers.onUnauthorized();
      }
      // "unreachable": let EventSource keep retrying on its own.
    });
  };

  return { close: () => source.close() };
}
