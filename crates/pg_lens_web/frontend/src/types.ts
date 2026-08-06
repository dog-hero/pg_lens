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

/**
 * One row of `pg_database` (U2 `databases` field, v0.13 web parity): the
 * databases available on this cluster, feeding the header's database
 * switcher (the web twin of the TUI's `d` picker).
 */
export interface DatabaseRow {
  name: string;
  /** `pg_database_size(datname)`; null when the connected role lacks
   * CONNECT privilege on that OTHER database (best-effort per row). */
  size_bytes: number | null;
}

export interface ActivityRow {
  pid: number;
  application_name: string;
  database: string;
  client: string;
  duration_secs: number;
  /** EXTRACT(epoch FROM (now() - xact_start)); null = no open transaction. */
  xact_age_secs: number | null;
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
  /** v0.14: `#[serde(default)]` in Rust — 0 on points from a pre-widening
   * history file loaded at startup, always populated on live ticks. */
  connections_total: number;
  /** 0.0..=100.0 (percentage, unlike `ServerVitals.cache_hit_ratio`); null
   * only on points predating v0.14's widening. */
  cache_hit_pct: number | null;
  /** 0.0..=100.0; null when the lock-capacity gauge itself was unavailable
   * that tick, not just on old data. */
  lock_pressure_pct: number | null;
  /** Oldest-transaction-ID age (Schema Lens data on its own slow cadence,
   * carried forward between collections — see `history.rs`'s doc comment).
   * Null before the first successful schema collection of a session. */
  oldest_xid_age: number | null;
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
  /** `pg_class.oid` — the size-growth ring's key (see pg_lens_core::schema_growth). */
  oid: number;
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
  /** v0.14: `total_bytes` delta over the last hour; `null` until the
   * poller's growth ring has 2+ samples for this table. Negative is a
   * valid, meaningful shrink (VACUUM FULL, TRUNCATE) — never clamped. */
  growth_1h_bytes: number | null;
  /** `growth_1h_bytes` as a percentage of the oldest sample; `null` under
   * the same conditions as `growth_1h_bytes`, or when that oldest sample
   * was itself 0 bytes. */
  growth_1h_pct: number | null;
  /** v0.15's partition collapsing: `true` when this row is a LEAF of a
   * native partitioned table (`pg_class.relispartition`) — hidden by
   * default in favor of the aggregated parent row (see `partition_count`),
   * shown only when the "show partitions" toggle is on. `false` for a
   * plain table AND for a parent's own synthesized row. */
  is_partition: boolean;
  /** The leaf's immediate parent oid (`pg_inherits.inhparent`) when
   * `is_partition` is true; `null` otherwise. Lets the frontend group
   * leaves under their parent for the drill-down section without a second
   * request. */
  parent_oid: number | null;
  /** `N` only on a SYNTHESIZED partition-parent row (aggregated over its
   * leaves via `pg_partition_tree`); `null` for every ordinary row
   * (including leaves) — this is the field that marks "this row IS a
   * partition-parent aggregate". */
  partition_count: number | null;
  /** v0.15's per-table lock indicator: this table's row count in
   * `pg_locks` at the most recent fast tick, folded on by the poller.
   * `null` before the first fold of a session, when that tick's
   * best-effort collection failed, OR when the table genuinely holds no
   * locks (a fresh join with nothing to report for it — see the Rust
   * `TableStatRow::lock_count` doc comment for why the two are
   * indistinguishable by design). */
  lock_count: number | null;
  /** Of `lock_count`, how many are NOT granted (a real waiter right now).
   * Same fold/freshness contract as `lock_count`. */
  lock_waiters: number | null;
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

/**
 * One orphaned two-phase-commit row (v0.9, `pg_prepared_xacts`): a
 * `PREPARE TRANSACTION` left dangling holds its locks and pins the
 * wraparound horizon indefinitely, with no session in `pg_stat_activity` to
 * blame — the classic silent incident that blocks vacuum forever.
 */
export interface PreparedXactRow {
  gid: string;
  owner: string;
  database: string;
  /** `EXTRACT(epoch FROM (now() - prepared))`. */
  age_seconds: number;
}

/**
 * Lock-table pressure gauge (v0.11): `pg_locks` count vs. the documented
 * shared-memory capacity formula (`max_locks_per_transaction * (max_connections
 * + max_prepared_transactions)`) — headroom before "out of shared memory,
 * you might need to increase max_locks_per_transaction". `capacity_slots` /
 * `used_fraction` are derived in Rust core (`lock_capacity::compute`), never
 * re-derived here.
 */
export interface LockCapacity {
  locks_held: number;
  max_locks_per_xact: number;
  max_connections: number;
  max_prepared_xacts: number;
  capacity_slots: number;
  /** 0.0..=1.0 */
  used_fraction: number;
}

/**
 * One idle connection (v0.11, `pg_stat_activity` `state = 'idle'`): a
 * backend holding a slot in the connection budget without doing anything —
 * the classic pool-exhaustion suspect (`connections_total` near
 * `max_connections` but few active). Ranked oldest-first by `idle_age_secs`.
 */
export interface IdleSessionRow {
  pid: number;
  application_name: string;
  database: string;
  client: string;
  username: string;
  /** `EXTRACT(epoch FROM (now() - state_change))`. */
  idle_age_secs: number;
}

/**
 * The Index Advisor's (F3) verdict for one index — serde external tagging:
 * unit variant `"Unused"`/`"None"`, struct variants `{ DuplicateExact: {
 * partner } }` / `{ DuplicatePrefix: { partner } }`. Computed in Rust core
 * (`index_advisor::classify`), never re-derived in the web frontend.
 */
export type IndexFinding =
  | "Invalid"
  | "Unused"
  | { DuplicateExact: { partner: string } }
  | { DuplicatePrefix: { partner: string } }
  | "None";

/** One row of the Index Advisor query (F3), current database only. */
export interface IndexRow {
  schema: string;
  table: string;
  name: string;
  index_bytes: number;
  idx_scan: number;
  idx_tup_read: number;
  idx_tup_fetch: number;
  is_unique: boolean;
  is_primary: boolean;
  is_exclusion: boolean;
  /** `pg_index.indisvalid` — false means a `CREATE INDEX CONCURRENTLY`
   * never finished building this index. */
  is_valid: boolean;
  /** `pg_index.indisready` — false means the index is not even being
   * maintained on writes yet. */
  is_ready: boolean;
  is_constraint: boolean;
  /** `pg_get_indexdef()` — the full `CREATE INDEX` statement, verbatim. */
  indexdef: string;
  finding: IndexFinding;
}

/** One column of the on-demand table detail (v0.15, `\d`-style overlay). */
export interface TableDetailColumn {
  name: string;
  /** `format_type(atttypid, atttypmod)` — e.g. `character varying(255)`. */
  data_type: string;
  not_null: boolean;
  /** `pg_get_expr(adbin, adrelid)` — `null` when the column has no default
   * AND is not a `GENERATED ... AS IDENTITY` column (those have no
   * `pg_attrdef` row either — see `identity` instead). Non-null for a plain
   * default OR for a `GENERATED ... AS (...) STORED` column's generation
   * expression, distinguished by `generated_stored`. */
  default: string | null;
  /** Friendly label from `pg_attribute.attidentity` — `"generated always as
   * identity"` / `"generated by default as identity"`, `null` for a
   * non-identity column. Never set alongside a `default`. */
  identity: string | null;
  /** `pg_attribute.attgenerated === 's'` — `default` holds a `GENERATED ...
   * AS (...) STORED` expression, not a plain default. */
  generated_stored: boolean;
}

/**
 * One constraint row of the on-demand table detail: either the table's OWN
 * constraint (`referencing_table: null`), or a foreign key on ANOTHER table
 * pointing back at this one (`referencing_table` set — "referenced by").
 */
export interface TableDetailConstraint {
  name: string;
  /** Friendly label: `PRIMARY KEY` / `FOREIGN KEY` / `UNIQUE` / `CHECK` /
   * `EXCLUDE` / `OTHER` — never the raw `pg_constraint.contype` letter. */
  kind: string;
  /** `pg_get_constraintdef(oid)` — verbatim. */
  definition: string;
  referencing_table: string | null;
}

/** One index definition of the on-demand table detail — `pg_get_indexdef`
 * verbatim output. */
export interface TableDetailIndex {
  name: string;
  definition: string;
}

/**
 * The full `\d`-style on-demand table detail (v0.15): columns, constraints
 * (own + referencing), and index definitions of ONE table, fetched only
 * when the Schema Lens detail is opened for it (`POST /api/schema/detail`)
 * — rides inside `DbSnapshot.table_detail`, never on the SSE stream's own
 * cadence. `error` is a best-effort failure reason (table dropped mid-
 * request, missing privilege) — never absent data, always something to
 * render.
 */
export interface TableDetail {
  oid: number;
  schema: string;
  name: string;
  collected_at_epoch_ms: number;
  columns: TableDetailColumn[];
  constraints: TableDetailConstraint[];
  indexes: TableDetailIndex[];
  error: string | null;
}

/** Slow-cadence Schema Lens collection; null until the first one lands. */
export interface SchemaSnapshot {
  collected_at_epoch_ms: number;
  tables: TableStatRow[];
  /** v0.15: the TRUE (uncapped) table count — `tables` itself is capped at
   * the configured `schema_table_limit` (default 200). `null` only before
   * the first successful slow collection of a session. */
  tables_total: number | null;
  table_bloat: BloatRow[];
  index_bloat: BloatRow[];
  /** null only before the first successful slow collection of a session. */
  vacuum_cluster_age: VacuumClusterAge | null;
  vacuum_tables: VacuumTableRow[];
  /** Index advisor rows (F3), same slow collection as `tables`. */
  indexes: IndexRow[];
  /** When the connected database's cumulative stats were last reset (F3
   * freshness header) — null only if the row vanished mid-query. */
  stats_reset_epoch_secs: number | null;
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
  /** Shared buffers dirtied/written by this statement (v0.14). */
  shared_blks_dirtied: number;
  shared_blks_written: number;
  /**
   * Temp-file (work_mem spill) blocks read/written, 8kB each (v0.14) —
   * the #1 query-tuning signal this row adds. `temp_blks_written` drives
   * the Temp column/sort/severity tint.
   */
  temp_blks_read: number;
  temp_blks_written: number;
  /**
   * Milliseconds spent on shared-buffer I/O; null when the
   * `track_io_timing` GUC is off — the raw counters read back as 0 in that
   * case, indistinguishable from "no I/O", so the core collapses both to
   * null rather than shipping a misleading zero.
   */
  blk_read_time_ms: number | null;
  blk_write_time_ms: number | null;
  /** Total WAL bytes generated; null on pg_stat_statements < 1.9. */
  wal_bytes: number | null;
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

/**
 * One row of pg_replication_slots (F2.5). Unlike WalSenderRow/WalReceiverRow
 * these exist on BOTH a primary and a standby, so they travel as their own
 * top-level DbSnapshot field rather than inside ReplicationInfo.
 */
export interface ReplicationSlotRow {
  slot_name: string;
  /** "physical" or "logical". */
  slot_type: string;
  active: boolean;
  /**
   * pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn); null during
   * recovery or when restart_lsn itself is null (unused logical slot).
   */
  retained_wal_bytes: number | null;
  /** "reserved" | "extended" | "unreserved" | "lost" (PG 13+). */
  wal_status: string | null;
  /** Headroom before max_slot_wal_keep_size is at risk; null when
   * unlimited/not applicable. */
  safe_wal_size: number | null;
}

/**
 * Checkpointer/bgwriter stats (F4), refreshed every fast tick alongside
 * `vitals` (not best-effort). Normalizes the PG17 `pg_stat_bgwriter` /
 * `pg_stat_checkpointer` catalog split into one shape; derived fields are
 * `null` on the first poll of a session (no delta window yet, same rule as
 * `ServerVitals.tps`).
 */
export interface CheckpointerStats {
  checkpoints_timed: number;
  checkpoints_req: number;
  checkpoint_write_time_ms: number;
  checkpoint_sync_time_ms: number;
  buffers_checkpoint: number;
  buffers_clean: number;
  maxwritten_clean: number;
  /** null on PG 17+ (moved to pg_stat_io). */
  buffers_backend: number | null;
  buffers_alloc: number;
  checkpoints_per_min_timed: number | null;
  checkpoints_per_min_req: number | null;
  buffers_checkpoint_per_sec: number | null;
  buffers_clean_per_sec: number | null;
  /** null when no delta window yet, OR when buffers_backend is absent. */
  buffers_backend_per_sec: number | null;
  avg_checkpoint_write_ms: number | null;
  avg_checkpoint_sync_ms: number | null;
  /**
   * requested / (requested + timed) checkpoints since the poller SESSION
   * began (not per-tick). null until a checkpoint has completed since the
   * session started.
   */
  requested_ratio_session: number | null;
}

/**
 * One row of the v0.16 I/O profile (`pg_stat_io`, PG 16+ only): one
 * backend_type x context combination's cumulative counters plus
 * poller-derived per-tick rates (same raw + derived split as
 * `CheckpointerStats`). Collected on the SLOW schema cadence — a
 * cluster-wide cumulative view, not per-backend.
 */
export interface IoStatRow {
  backend_type: string;
  context: string;
  reads: number;
  writes: number;
  writebacks: number;
  extends: number;
  hits: number;
  evictions: number;
  reuses: number;
  fsyncs: number;
  /** Average ms/read (read_time / reads, both cumulative). null when
   * track_io_timing is off (indistinguishable from 0 otherwise — same
   * convention as `StatementRow.blk_read_time_ms`) or reads is 0. */
  avg_read_ms: number | null;
  avg_write_ms: number | null;
  /** null on the first collection of a session, OR across a stats reset
   * (the counter went backwards) — never a negative rate. */
  reads_per_sec: number | null;
  writes_per_sec: number | null;
  /** hits / (hits + reads), delta-based when a window exists. null only
   * when the denominator is 0. */
  hit_ratio: number | null;
}

/**
 * WAL generation rate (v0.16, `pg_stat_wal`, PG 14+ only): cluster-wide
 * cumulative WAL counters plus poller-derived per-tick rates, the same
 * raw + derived split as `CheckpointerStats`. Collected on the FAST tick,
 * best-effort like `replication` (null on PG < 14, a restricted role, or
 * any collection failure — never an error state, just absent).
 */
export interface WalStats {
  wal_records: number;
  wal_fpi: number;
  wal_bytes: number;
  /** Times a backend had to write WAL data directly because every WAL
   * buffer was full — a real tuning signal (wal_buffers too small) when
   * nonzero; see `wal_buffers_full_delta` for "is it climbing right now". */
  wal_buffers_full: number;
  /** null when track_wal_io_timing is off (indistinguishable from 0
   * otherwise — same convention as `IoStatRow.avg_read_ms`). */
  wal_write_time_ms: number | null;
  wal_sync_time_ms: number | null;
  /** null on the first collection of a session, OR across a stats reset
   * (the counter went backwards) — never a negative rate. */
  wal_bytes_per_sec: number | null;
  wal_records_per_sec: number | null;
  /** wal_buffers_full's delta over this tick's window. 0 means no NEW
   * buffer-full events since the last tick (calm, even if the cumulative
   * counter is nonzero from history); a positive value means it is
   * actively climbing right now. Same null rules as the rate fields. */
  wal_buffers_full_delta: number | null;
}

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
   * pg_replication_slots rows (F2.5), refreshed every fast tick,
   * best-effort like `replication`: null when the collection failed this
   * tick, an empty array when it succeeded and simply found no slots (the
   * common, calm case — rendered as no extra rows, never an error).
   */
  replication_slots: ReplicationSlotRow[] | null;
  /**
   * In-flight vacuum progress (F2), refreshed every fast tick, best-effort:
   * null when the collection failed this tick (restricted role, hidden
   * view, ...); an empty array means it succeeded and found nothing running
   * — the common, calm case, never rendered as an error.
   */
  vacuum_progress: VacuumProgressRow[] | null;
  /**
   * Checkpointer/bgwriter stats (F4), refreshed every fast tick — NOT
   * best-effort. null only before the first successful poll of a session.
   */
  checkpointer: CheckpointerStats | null;
  /**
   * Orphaned two-phase-commit watch (v0.9), refreshed every fast tick,
   * best-effort like `vacuum_progress`: null when the collection failed
   * this tick, an empty array when it succeeded and simply found no
   * dangling prepared transaction (the overwhelmingly common, calm case).
   */
  prepared_xacts: PreparedXactRow[] | null;
  /**
   * Lock-table pressure gauge (v0.11), refreshed every fast tick,
   * best-effort like `prepared_xacts`: null when the collection failed this
   * tick (restricted role, a renamed GUC, ...) — otherwise always present,
   * since every cluster has a lock table (no "found nothing" empty case).
   */
  lock_capacity: LockCapacity | null;
  /**
   * Idle connection / connection-age census (v0.11), refreshed every fast
   * tick, best-effort like `prepared_xacts`: null when the collection failed
   * this tick, an empty array when it succeeded and simply found no idle
   * sessions (a fully busy or freshly-started server — calm, never an
   * error). Oldest (most suspect) first, capped at 100 rows.
   */
  idle_sessions: IdleSessionRow[] | null;
  /**
   * The databases available on this cluster (U2/v0.13), best-effort: null
   * when the connected role can't list `pg_database` (restricted role) or
   * on a single-database deployment — the header switcher hides itself in
   * that case and just shows `vitals.database`.
   */
  databases: DatabaseRow[] | null;
  /**
   * On-demand `\d`-style table detail (v0.15), stamped by the poller after
   * a `POST /api/schema/detail` request — `null` before any request this
   * session, or after a different table's fetch superseded it (match on
   * `table_detail.oid` before rendering, since the response can lag a tick
   * or two behind the click).
   */
  table_detail: TableDetail | null;
  /**
   * I/O profile (v0.16, `pg_stat_io`), collected on the SAME slow schema
   * cadence as `schema`/`statements`. `null` on PG < 16 (the view does not
   * exist — absent, not an error) OR before the first slow collection of a
   * session; an empty array means the collection succeeded and simply found
   * no nonzero row (a genuinely idle server).
   */
  io_stats: IoStatRow[] | null;
  /**
   * WAL generation rate (v0.16, `pg_stat_wal`), refreshed every fast tick,
   * best-effort like `replication`: null on PG < 14 (the view does not
   * exist — absent, not an error), a restricted role, or when the
   * collection failed this tick.
   */
  wal: WalStats | null;
  status: PollerStatus;
  last_admin_action: AdminActionResult | null;
}
