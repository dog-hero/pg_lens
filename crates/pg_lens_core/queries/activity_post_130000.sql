-- Micro Lens activity (PG 13.x).
-- Same as activity_post_140000.sql minus query_id, which only exists on
-- PG >= 14 (pg_activity ships the same NULL::int8 placeholder in its
-- get_pg_activity_post_130000.sql).
-- Note: EXTRACT returns float8 on PG 13; the ::float8 cast is a no-op kept
-- so both variants produce identical column types.
-- xact_age_seconds added (v0.9, idle-in-transaction / xact-age hunter): same
-- derivation as the 14+ variant.
SELECT
      a.pid AS pid,
      a.application_name AS application_name,
      a.datname AS database,
      a.client_addr::text AS client,
      EXTRACT(epoch FROM (NOW() - a.query_start))::float8 AS duration,
      CASE WHEN a.xact_start IS NULL THEN NULL
           ELSE EXTRACT(epoch FROM (NOW() - a.xact_start))::float8
      END AS xact_age_seconds,
      CASE WHEN a.wait_event IS NULL THEN NULL
           ELSE a.wait_event_type || ':' || a.wait_event
      END AS wait,
      a.usename AS usename,
      a.state AS state,
      a.query AS query,
      coalesce(a.leader_pid, a.pid) AS query_leader_pid,
      coalesce(a.backend_type = 'parallel worker', false) AS is_parallel_worker,
      NULL::int8 AS query_id
 FROM
      pg_stat_activity a
 WHERE
      a.state <> 'idle'
  AND a.pid <> pg_catalog.pg_backend_pid()
ORDER BY
      EXTRACT(epoch FROM (NOW() - a.query_start)) DESC NULLS LAST;
