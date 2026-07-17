-- Query Lens, pg_stat_statements extension >= 1.9 (PG 14+ default install,
-- but tracked by EXTENSION version — see statements.sql's header for the
-- full tier rundown and why one Rust parser serves all three files).
--
-- 1.9 added `wal_bytes` (per-statement WAL volume) — the only diff from the
-- base (1.8) variant. Column names otherwise unchanged from 1.8 (the
-- `blk_read_time`/`blk_write_time` rename arrives in 1.11, a separate tier).
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
      s.wal_bytes::int8 AS wal_bytes,
      current_setting('track_io_timing')::boolean AS track_io_timing_on
 FROM pg_stat_statements AS s
 JOIN pg_database AS d ON d.oid = s.dbid
 LEFT JOIN pg_roles AS r ON r.oid = s.userid
WHERE d.datname = current_database()
ORDER BY s.total_exec_time DESC
LIMIT 100;
