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

/** Cluster-wide XID wraparound headline (F2), `age(datfrozenxid)`. */
export interface VacuumClusterAge {
  max_age_xids: number;
  worst_database: string;
}

/** One table's XID age + dead-tuple ratio ("vacuum debt"), F2. */
export interface VacuumTableRow {
  schema: string;
  name: string;
  age_xids: number;
  n_dead_tup: number;
  n_live_tup: number;
}

/** One in-flight `pg_stat_progress_vacuum` row (F2). */
export interface VacuumProgressRow {
  pid: number;
  relation: string;
  phase: string;
  heap_blks_total: number;
  heap_blks_scanned: number;
}

/** Slow-cadence Schema Lens collection; null until the first one lands. */
export interface SchemaSnapshot {
  collected_at_epoch_ms: number;
  tables: TableStatRow[];
  table_bloat: BloatRow[];
  index_bloat: BloatRow[];
  /** null only before the first successful slow collection of a session. */
  vacuum_cluster_age: VacuumClusterAge | null;
  vacuum_tables: VacuumTableRow[];
  status: SchemaStatus;
}

/**
 * Same serde external tagging; `Unavailable` is the calm "extension
 * missing / too old" state (its string is the human-readable reason/hint).
 */
export type StatementsStatus =
  | "Ok"
  | { Unavailable: string }
  | { Error: string };

/** One pg_stat_statements row (Query Lens), current database only. */
export interface StatementRow {
  /**
   * queryid as a STRING: the raw int8 can exceed Number.MAX_SAFE_INTEGER,
   * so the core ships it as text. null = NULL queryid.
   */
  query_id: string | null;
  query: string;
  username: string;
  calls: number;
  total_exec_ms: number;
  mean_exec_ms: number;
  rows: number;
  shared_blks_hit: number;
  shared_blks_read: number;
}

/**
 * Slow-cadence statements collection (shares the schema tick); null until
 * the first one lands.
 */
export interface StatementsSnapshot {
  collected_at_epoch_ms: number;
  statements: StatementRow[];
  status: StatementsStatus;
}

/** One streaming replica of a primary (pg_stat_replication). */
export interface WalSenderRow {
  application_name: string;
  client: string;
  state: string;
  sync_state: string;
  replay_lag_bytes: number | null;
  replay_lag_secs: number | null;
}

/** The standby side (pg_stat_wal_receiver + last replay position). */
export interface WalReceiverRow {
  status: string;
  sender_host: string | null;
  sender_port: number | null;
  replay_lag_bytes: number | null;
  replay_lag_secs: number | null;
}

/**
 * Replication role & topology (externally-tagged, mirroring the Rust enum):
 * a primary lists its replicas, a standby carries its WAL receiver.
 */
export type ReplicationInfo =
  | { Primary: { senders: WalSenderRow[] } }
  | { Standby: { receiver: WalReceiverRow | null } };

/** Result of an admin action, stamped by the poller inside every snapshot
 * until superseded; frontends dedupe by `at_epoch_ms`. Serde shapes:
 * kind = "Cancel"|"Terminate", outcome = {Signalled:bool}|{Error:string}. */
export interface AdminActionResult {
  kind: "Cancel" | "Terminate";
  pid: number;
  outcome: { Signalled: boolean } | { Error: string };
  at_epoch_ms: number;
}

export interface DbSnapshot {
  vitals: ServerVitals;
  activity: ActivityRow[];
  locks: LockRow[];
  history: SnapshotHistory;
  schema: SchemaSnapshot | null;
  statements: StatementsSnapshot | null;
  replication: ReplicationInfo | null;
  /**
   * In-flight vacuum progress (F2), refreshed every fast tick, best-effort:
   * null when the collection failed this tick (restricted role, hidden
   * view, ...); an empty array means it succeeded and found nothing running
   * — the common, calm case, never rendered as an error.
   */
  vacuum_progress: VacuumProgressRow[] | null;
  status: PollerStatus;
  last_admin_action: AdminActionResult | null;
}
