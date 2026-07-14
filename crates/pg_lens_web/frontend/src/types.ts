// Mirrors the serde::Serialize output of pg_lens_core::models (verified
// against a live `GET /api/snapshot` payload — do not rename fields here
// without changing the Rust structs).

export interface ServerVitals {
  server_version: string;
  uptime_secs: number;
  connections_total: number;
  max_connections: number;
  active: number;
  idle: number;
  idle_in_transaction: number;
  waiting: number;
  tps: number;
  /** 0.0..=1.0 */
  cache_hit_ratio: number;
  tup_returned: number;
  tup_fetched: number;
  temp_files: number;
  temp_bytes: number;
  deadlocks: number;
}

export interface ActivityRow {
  pid: number;
  application_name: string;
  database: string;
  client: string;
  duration_secs: number;
  wait_event: string | null;
  username: string;
  state: string;
  query: string;
  query_leader_pid: number;
  is_parallel_worker: boolean;
  query_id: number | null;
}

export interface LockRow {
  pid: number;
  blocked_by: number[];
  mode: string | null;
  locktype: string | null;
  relation: string | null;
  duration_secs: number;
  query: string;
}

export interface HistoryPoint {
  epoch_ms: number;
  tps: number;
  active_sessions: number;
}

export interface SnapshotHistory {
  cap: number;
  points: HistoryPoint[];
}

/** serde external tagging: unit variants are strings, tuple variants maps. */
export type PollerStatus = "Ok" | "Connecting" | { Error: string };

export interface DbSnapshot {
  vitals: ServerVitals;
  activity: ActivityRow[];
  locks: LockRow[];
  history: SnapshotHistory;
  status: PollerStatus;
}
