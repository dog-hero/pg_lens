// Header server / cluster switcher (v0.21): web parity for runtime
// cluster switching from services.toml.
//
// Posts to `/api/server/switch`, which asks the poller to disconnect
// from the current cluster, reset ring buffers/stats, and reconnect to
// the selected server.

import type { ServiceSummary } from "./actions.ts";

/** One `<option>`'s label: name plus best-effort connection coordinates. */
export function serverOptionLabel(server: ServiceSummary): string {
  const host = server.host ?? "localhost";
  const port = server.port ?? 5432;
  const user = server.user ?? "-";
  return `${server.name} (${user}@${host}:${port})`;
}

/**
 * Whether the switcher has multiple servers to switch between:
 * at least 2 servers present in services.toml.
 */
export function hasSwitchableServers(servers: ServiceSummary[] | null): boolean {
  return servers !== null && servers.length >= 2;
}

/**
 * Rebuilds the `<select>`'s options from the server list, selecting `currentServer`.
 * Hides the element when no servers are available or only 1 exists.
 */
export function populateServerSwitcher(
  select: HTMLSelectElement,
  servers: ServiceSummary[] | null,
  currentServer: string | null,
): void {
  if (!hasSwitchableServers(servers)) {
    select.hidden = true;
    return;
  }
  const options = (servers as ServiceSummary[]).map((s) => {
    const option = document.createElement("option");
    option.value = s.name;
    option.textContent = serverOptionLabel(s);
    option.selected = s.name === currentServer;
    return option;
  });
  select.replaceChildren(...options);
  // Show if 2 or more, or if configured with at least 1 server
  select.hidden = false;
}
