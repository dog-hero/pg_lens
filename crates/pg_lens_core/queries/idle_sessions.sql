-- Idle connection / connection-age census (v0.11): the Micro Lens activity
-- query filters OUT `state <> 'idle'` sessions, so a classic pool-exhaustion
-- incident (connections_total near max_connections but few active) is
-- undiagnosable today — WHICH idle connections (app/host/user) are eating
-- the budget? Own query (Option B in the v0.11 discovery), separate from
-- `activity_post_*.sql`, so the hot activity table's shape/consumers (the
-- Micro Lens active-session view, blocking chain, waits aggregation, web
-- table) are untouched, and a pool-heavy server with hundreds of idle
-- sessions cannot bloat the per-tick activity payload — capped at 100 rows,
-- oldest (most suspect) first.
--
-- Deliberately `state = 'idle'` only (plain idle, backend not inside a
-- transaction) — a backend with an open transaction is already covered by
-- the v0.9 idle-in-transaction / xact-age hunter via `xact_age_seconds` in
-- the activity query, which answers a different question (an open
-- transaction holding locks, not a spare pooled connection).
--
-- client_addr cast to text (tokio-postgres has no default `inet` mapping,
-- same as `activity_post_*.sql`); EXTRACT(epoch...) cast to float8 (numeric
-- on PG >= 14, float8 on 13 — the well-known type trap). Version-independent
-- 13+ (state_change is stable across the whole supported range) — no
-- post_NNNNNN variant needed.
SELECT
      a.pid AS pid,
      a.application_name AS application_name,
      a.datname AS database,
      a.client_addr::text AS client,
      a.usename AS usename,
      EXTRACT(epoch FROM (now() - a.state_change))::float8 AS idle_age_seconds
 FROM
      pg_stat_activity a
 WHERE
      a.state = 'idle'
  AND a.pid <> pg_catalog.pg_backend_pid()
ORDER BY
      a.state_change ASC NULLS LAST
LIMIT 100;
