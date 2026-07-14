-- Micro Lens blocking info (works on PG >= 13; every function used here
-- exists since 9.6, so queries.rs selects this same file for PG 13).
--
-- pg_activity's get_blocking_post_140000.sql lists the *blocking* sessions
-- through a triple pg_locks self-join (transactionid / virtualxid /
-- relation). pg_lens needs the opposite mapping — which pids are blocked and
-- by whom — so this uses the simpler documented alternative from PLAN.md /
-- the phase brief: pg_stat_activity filtered on pg_blocking_pids(), joined
-- to each blocked backend's single not-granted pg_locks entry for the lock
-- mode / type / relation (a backend waits on at most one lock at a time).
SELECT
      a.pid AS pid,
      pg_blocking_pids(a.pid) AS blocked_by,
      l.mode AS mode,
      l.locktype AS locktype,
      l.relation::regclass::text AS relation,
      EXTRACT(epoch FROM (NOW() - a.query_start))::float8 AS duration,
      a.query AS query
 FROM
      pg_stat_activity a
      LEFT OUTER JOIN pg_locks l ON l.pid = a.pid AND NOT l.granted
 WHERE
      cardinality(pg_blocking_pids(a.pid)) > 0
  AND a.pid <> pg_catalog.pg_backend_pid();
