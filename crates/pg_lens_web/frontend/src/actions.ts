// Authenticated control actions (Fase #24 web parity): schema recollect and
// admin cancel/terminate. All use the Authorization header (fetch can set it),
// keeping the token out of URLs. Admin requires the server to have a token
// configured — it answers 403 otherwise, surfaced to the caller.

function authHeaders(token: string | null): HeadersInit {
  return token === null ? {} : { Authorization: `Bearer ${token}` };
}

/**
 * GET /api/config — small, non-secret feature flags (currently just
 * `read_only`). Used to grey out admin controls; this is defense in depth
 * ONLY — the server refuses `/api/admin/*` itself regardless of what this
 * endpoint reports or whether the client even calls it. Best-effort: a
 * fetch failure is treated as "not read-only" (fail open on the UI side —
 * the server-side gate still holds either way).
 */
export async function fetchConfig(token: string | null): Promise<{ readOnly: boolean }> {
  try {
    const res = await fetch("/api/config", { headers: authHeaders(token) });
    if (!res.ok) return { readOnly: false };
    const body = (await res.json()) as { read_only?: boolean };
    return { readOnly: body.read_only === true };
  } catch {
    return { readOnly: false };
  }
}

/** POST /api/schema/refresh — trigger a Schema Lens recollect. */
export async function requestSchemaRefresh(token: string | null): Promise<boolean> {
  try {
    const res = await fetch("/api/schema/refresh", {
      method: "POST",
      headers: authHeaders(token),
    });
    return res.ok;
  } catch {
    return false;
  }
}

export type AdminKind = "cancel" | "terminate";

export interface AdminResult {
  ok: boolean;
  /** HTTP status (0 = network error) — 403 means the server has no token. */
  status: number;
}

/** POST /api/admin/{cancel,terminate}/{pid}. */
export async function requestAdmin(
  token: string | null,
  kind: AdminKind,
  pid: number,
): Promise<AdminResult> {
  try {
    const res = await fetch(`/api/admin/${kind}/${pid}`, {
      method: "POST",
      headers: authHeaders(token),
    });
    return { ok: res.ok, status: res.status };
  } catch {
    return { ok: false, status: 0 };
  }
}
