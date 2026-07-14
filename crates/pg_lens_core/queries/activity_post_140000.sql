-- Micro Lens activity (PG >= 14).
-- Adapted from dalibo/pg_activity: pgactivity/queries/get_pg_activity_post_140000.sql
-- Adaptations for pg_lens:
--   * {duration_column} fixed to query_start; {min_duration} fixed to 0 and
--     {dbname_filter} removed (pg_activity's Python placeholders).
--   * xmin / encoding columns dropped (unused by pg_lens models), which also
--     drops the pg_database join.
--   * client_addr cast to text (tokio-postgres has no default `inet` mapping).
--   * duration cast to float8 (EXTRACT returns numeric on PG >= 14).
--   * wait combines wait_event_type:wait_event for display.
--   * usename aliased as usename (not `user`) to keep row extraction plain.
SELECT
      a.pid AS pid,
      a.application_name AS application_name,
      a.datname AS database,
      a.client_addr::text AS client,
      EXTRACT(epoch FROM (NOW() - a.query_start))::float8 AS duration,
      CASE WHEN a.wait_event IS NULL THEN NULL
           ELSE a.wait_event_type || ':' || a.wait_event
      END AS wait,
      a.usename AS usename,
      a.state AS state,
      a.query AS query,
      coalesce(a.leader_pid, a.pid) AS query_leader_pid,
      coalesce(a.backend_type = 'parallel worker', false) AS is_parallel_worker,
      a.query_id AS query_id
 FROM
      pg_stat_activity a
 WHERE
      a.state <> 'idle'
  AND a.pid <> pg_catalog.pg_backend_pid()
ORDER BY
      EXTRACT(epoch FROM (NOW() - a.query_start)) DESC NULLS LAST;
