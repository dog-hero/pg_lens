-- Macro Lens server vitals (PG >= 13), one row.
-- Per PLAN.md's data-layer mapping table:
--   * pg_stat_database aggregate — cumulative counters; TPS and cache hit
--     ratio are derived by delta in the poller, never in SQL.
--   * pg_stat_activity count-by-state (client backends only, so the total
--     compares meaningfully against max_connections). "waiting" counts
--     non-idle sessions with a wait_event; idle sessions always wait on
--     Client:ClientRead and would make the counter meaningless.
--   * max_connections / uptime / server version. server_version is trimmed
--     to its numeric part (Docker builds append "(Debian ...)").
SELECT
      d.xact_commit,
      d.xact_rollback,
      d.blks_hit,
      d.blks_read,
      d.tup_returned,
      d.tup_fetched,
      d.temp_files,
      d.temp_bytes,
      d.deadlocks,
      s.connections_total,
      s.active,
      s.idle,
      s.idle_in_transaction,
      s.waiting,
      current_setting('max_connections')::int4 AS max_connections,
      EXTRACT(epoch FROM (NOW() - pg_postmaster_start_time()))::float8 AS uptime_secs,
      split_part(current_setting('server_version'), ' ', 1) AS server_version,
      current_database()::text AS database,
      pg_is_in_recovery() AS is_in_recovery
 FROM
      (SELECT
            coalesce(sum(xact_commit), 0)::int8 AS xact_commit,
            coalesce(sum(xact_rollback), 0)::int8 AS xact_rollback,
            coalesce(sum(blks_hit), 0)::int8 AS blks_hit,
            coalesce(sum(blks_read), 0)::int8 AS blks_read,
            coalesce(sum(tup_returned), 0)::int8 AS tup_returned,
            coalesce(sum(tup_fetched), 0)::int8 AS tup_fetched,
            coalesce(sum(temp_files), 0)::int8 AS temp_files,
            coalesce(sum(temp_bytes), 0)::int8 AS temp_bytes,
            coalesce(sum(deadlocks), 0)::int8 AS deadlocks
         FROM pg_stat_database) AS d,
      (SELECT
            count(*)::int4 AS connections_total,
            (count(*) FILTER (WHERE state = 'active'))::int4 AS active,
            (count(*) FILTER (WHERE state = 'idle'))::int4 AS idle,
            (count(*) FILTER (WHERE state LIKE 'idle in transaction%'))::int4 AS idle_in_transaction,
            (count(*) FILTER (WHERE wait_event IS NOT NULL AND state <> 'idle'))::int4 AS waiting
         FROM pg_stat_activity
        WHERE backend_type = 'client backend') AS s;
