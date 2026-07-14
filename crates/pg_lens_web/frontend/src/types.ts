// Mirrors the serde::Serialize output of pg_lens_core::models (verified
// against a live `GET /api/snapshot` payload — do not rename fields here
// without changing the Rust structs).

export interface ServerVitals {
  server_version: string;
  /** Database the DSN connected to — the Schema Lens is per-database. */
  database: string;
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

/** Same serde external tagging as PollerStatus (verified: `"status": "Ok"`). */
export type SchemaStatus = "Ok" | { Error: string };

export interface TableStatRow {
  schema: string;
  name: string;
  total_bytes: number;
  table_bytes: number;
  index_bytes: number;
  seq_scan: number;
  seq_tup_read: number;
  idx_scan: number | null;
  idx_tup_fetch: number | null;
  n_tup_ins: number;
  n_tup_upd: number;
  n_tup_del: number;
  n_tup_hot_upd: number;
  n_live_tup: number;
  n_dead_tup: number;
  n_mod_since_analyze: number;
  n_ins_since_vacuum: number;
  last_vacuum_epoch_secs: number | null;
  last_autovacuum_epoch_secs: number | null;
  last_analyze_epoch_secs: number | null;
  last_autoanalyze_epoch_secs: number | null;
  vacuum_count: number;
  autovacuum_count: number;
  analyze_count: number;
  autoanalyze_count: number;
}

/** ioguix-estimated bloat of a table or btree index. */
export interface BloatRow {
  schema: string;
  name: string;
  /** Owning table — set for index rows, null for table rows. */
  table: string | null;
  real_bytes: number;
  /** null when `is_na` — never render a made-up number. */
  bloat_bytes: number | null;
  bloat_pct: number | null;
  fillfactor: number | null;
  /** Estimate is not reliable (e.g. `name` columns, stats missing). */
  is_na: boolean;
}

/** Slow-cadence Schema Lens collection; null until the first one lands. */
export interface SchemaSnapshot {
  collected_at_epoch_ms: number;
  tables: TableStatRow[];
  table_bloat: BloatRow[];
  index_bloat: BloatRow[];
  status: SchemaStatus;
}

export interface DbSnapshot {
  vitals: ServerVitals;
  activity: ActivityRow[];
  locks: LockRow[];
  history: SnapshotHistory;
  schema: SchemaSnapshot | null;
  status: PollerStatus;
}
