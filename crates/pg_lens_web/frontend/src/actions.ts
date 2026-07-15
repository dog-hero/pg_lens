// Authenticated control actions (Fase #24 web parity): schema recollect and
// admin cancel/terminate. All use the Authorization header (fetch can set it),
// keeping the token out of URLs. Admin requires the server to have a token
// configured — it answers 403 otherwise, surfaced to the caller.

function authHeaders(token: string | null): HeadersInit {
  return token === null ? {} : { Authorization: `Bearer ${token}` };
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
