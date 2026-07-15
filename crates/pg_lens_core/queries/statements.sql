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
-- Type notes (tokio-postgres):
--   * queryid is int8 but ships ::text — an i64 can exceed JS
--     Number.MAX_SAFE_INTEGER, and the web frontend consumes this as JSON.
--   * rolname is `name` — cast ::text for a clean String map.
--   * calls/rows/shared_blks_* are int8 already; exec times float8 already;
--     casts kept explicit per the repo's type-trap convention.
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
      s.shared_blks_read::int8 AS shared_blks_read
 FROM pg_stat_statements AS s
 JOIN pg_database AS d ON d.oid = s.dbid
 LEFT JOIN pg_roles AS r ON r.oid = s.userid
WHERE d.datname = current_database()
ORDER BY s.total_exec_time DESC
LIMIT 100;
