-- Query Lens: normalized statement stats from pg_stat_statements.
-- Runs on the SLOW cadence only (shares the Schema Lens tick — never the
-- 2s activity tick), and is only PREPARED when the extension is installed
-- at version >= 1.8 (see db::statements_availability): the exec-time column
-- names (total_exec_time & friends) arrived in 1.8, the version PG 13
-- ships. Column availability follows the EXTENSION version, not the server
-- version — an upgraded cluster can carry an older extension.
--
-- pg_stat_statements is CLUSTER-wide; this lens deliberately filters to the
-- connected database (dbid = current database's oid) for consistency with
-- the per-database Schema Lens — the lens footer says so.
--
-- v0.14 I/O & temp-spill profile: this is the ext-1.8-only variant (columns
-- present since 1.8, base of the extension gate). Two sibling files widen
-- the shape as the EXTENSION version climbs — chosen at session-init time
-- by `queries::statements_sql_for_extension` (a SEPARATE decision from
-- `queries::for_version`'s server-version selection, since these column
-- renames/additions track the extension, not the server):
--   * `statements_ext_1_9.sql` (ext >= 1.9): adds `wal_bytes`, the
--     per-statement WAL volume.
--   * `statements_ext_1_11.sql` (ext >= 1.11, the pg_stat_statements
--     release that shipped with PG 17): `blk_read_time`/`blk_write_time`
--     were renamed `shared_blk_read_time`/`shared_blk_write_time` (a new
--     `local_blk_*` pair was split out alongside them).
-- All three files alias their output to the SAME column names
-- (`blk_read_time_ms`/`blk_write_time_ms`/`wal_bytes`), so ONE Rust parser
-- (`db::statement_from_row`) serves every tier — same trick as the
-- checkpointer's PG17 catalog split (`bgwriter_post_170000.sql`). This
-- file sends a typed `NULL::int8 AS wal_bytes` for the tier that lacks it.
--
-- `track_io_timing_on` rides along as a per-row scalar (constant across
-- every row — `current_setting` is evaluated once, the planner does not
-- re-run it per tuple): when the GUC is off, `blk_read_time`/`blk_write_time`
-- read back as 0, indistinguishable from "no time spent" — the Rust parser
-- collapses both to `None` in that case rather than shipping a misleading 0.
--
-- Type notes (tokio-postgres):
--   * queryid is int8 but ships ::text — an i64 can exceed JS
--     Number.MAX_SAFE_INTEGER, and the web frontend consumes this as JSON.
--   * rolname is `name` — cast ::text for a clean String map.
--   * calls/rows/shared_blks_*/temp_blks_* are int8 already; exec times and
--     blk_*_time are float8 already; casts kept explicit per the repo's
--     type-trap convention.
--
-- LIMIT guards the payload: the top 100 by total execution time is what
-- the lens can usefully show.
SELECT
      s.queryid::text AS queryid,
      s.query AS query,
      r.rolname::text AS usename,
      s.calls::int8 AS calls,
      s.total_exec_time::float8 AS total_exec_time,
      s.mean_exec_time::float8 AS mean_exec_time,
      s.rows::int8 AS rows,
      s.shared_blks_hit::int8 AS shared_blks_hit,
      s.shared_blks_read::int8 AS shared_blks_read,
      s.shared_blks_dirtied::int8 AS shared_blks_dirtied,
      s.shared_blks_written::int8 AS shared_blks_written,
      s.temp_blks_read::int8 AS temp_blks_read,
      s.temp_blks_written::int8 AS temp_blks_written,
      s.blk_read_time::float8 AS blk_read_time_ms,
      s.blk_write_time::float8 AS blk_write_time_ms,
      NULL::int8 AS wal_bytes,
      current_setting('track_io_timing')::boolean AS track_io_timing_on
 FROM pg_stat_statements AS s
 JOIN pg_database AS d ON d.oid = s.dbid
 LEFT JOIN pg_roles AS r ON r.oid = s.userid
WHERE d.datname = current_database()
ORDER BY s.total_exec_time DESC
LIMIT 100;
